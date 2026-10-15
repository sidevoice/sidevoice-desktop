//! The app's private pairing file, the machine's shared model root, and the confirmed reset.
//! Only the current room page can ask; no bridge method accepts a filesystem path.

use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Manager, Webview};

use crate::AppState;

type Answer = Result<Value, Value>;

fn refused(key: &str) -> Value { json!({ "key": key }) }

fn caller(app: &AppHandle, webview: &Webview) -> Result<(), Value> {
    crate::room_page(app, webview).map_err(|_| refused("reset.unauthorized"))
}

fn desktop_dir(app: &AppHandle) -> Result<PathBuf, Value> {
    app.path().app_data_dir().map_err(|_| refused("reset.unavailable"))
}

pub fn shared_model_root() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    { Some(PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/Sidevoice/Models")) }
    #[cfg(target_os = "linux")]
    { Some(std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")).join("sidevoice-shared/models")) }
    #[cfg(target_os = "windows")]
    { Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Sidevoice/Models")) }
}

fn pairings_file(app: &AppHandle) -> Result<PathBuf, Value> { Ok(desktop_dir(app)?.join("pairings.json")) }

fn checked_pairings(value: &Value) -> bool {
    let Some(rows) = value.get("pairings").and_then(Value::as_array) else { return false };
    if rows.len() > 32 || serde_json::to_vec(value).is_err_or(|bytes| bytes.len() > 64 * 1024) { return false }
    rows.iter().all(|row| {
        row.get("local").is_none()
            && ["fp", "public_key", "device_id", "token"].iter().all(|field| row.get(*field).and_then(Value::as_str).is_some_and(|s| !s.is_empty() && s.len() <= 4096))
            && row.get("urls").and_then(Value::as_array).is_some_and(|urls| urls.len() <= 16 && urls.iter().all(|url| url.as_str().is_some_and(|s| s.len() <= 2048)))
    })
}

fn read_pairings(path: &Path) -> Answer {
    match fs::read(path) {
        Ok(bytes) if bytes.len() <= 64 * 1024 => serde_json::from_slice::<Value>(&bytes).ok().filter(checked_pairings).ok_or_else(|| refused("data.pairings-invalid")),
        Ok(_) => Err(refused("data.pairings-invalid")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({"pairings": []})),
        Err(_) => Err(refused("data.pairings-read")),
    }
}

fn write_pairings(path: &Path, value: &Value) -> Answer {
    if !checked_pairings(value) { return Err(refused("data.pairings-invalid")) }
    let parent = path.parent().ok_or_else(|| refused("data.pairings-write"))?;
    fs::create_dir_all(parent).map_err(|_| refused("data.pairings-write"))?;
    if fs::symlink_metadata(parent).is_ok_and(|m| m.file_type().is_symlink()) ||
        fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) { return Err(refused("reset.unsafe")) }
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    let mut file = options.open(&tmp).map_err(|_| refused("data.pairings-write"))?;
    let result = (|| {
        file.write_all(&serde_json::to_vec(value).map_err(|_| refused("data.pairings-invalid"))?).map_err(|_| refused("data.pairings-write"))?;
        file.sync_all().map_err(|_| refused("data.pairings-write"))?;
        fs::rename(&tmp, path).map_err(|_| refused("data.pairings-write"))?;
        Ok(Value::Null)
    })();
    if result.is_err() { let _ = fs::remove_file(&tmp); }
    result
}

#[tauri::command]
pub fn data_pairings_read(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    read_pairings(&pairings_file(&app)?)
}

#[tauri::command]
pub fn data_pairings_write(app: AppHandle, webview: Webview, value: Value) -> Answer {
    caller(&app, &webview)?;
    write_pairings(&pairings_file(&app)?, &value)
}

#[tauri::command]
pub fn data_reset_report(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    #[cfg(target_os = "macos")]
    {
        let path = report_path().ok_or_else(|| refused("reset.unavailable"))?;
        match fs::read(&path) {
            Ok(bytes) if bytes.len() <= 16 * 1024 => {
                let value = serde_json::from_slice(&bytes).map_err(|_| refused("reset.failed"))?;
                fs::remove_file(path).map_err(|_| refused("reset.failed"))?;
                Ok(value)
            }
            Ok(_) => Err(refused("reset.failed")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
            Err(_) => Err(refused("reset.failed")),
        }
    }
    #[cfg(not(target_os = "macos"))]
    { Err(refused("reset.unavailable")) }
}

#[tauri::command]
pub async fn data_delete_all(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    #[cfg(not(target_os = "macos"))]
    { let _ = app; return Err(refused("reset.unavailable")); }
    #[cfg(target_os = "macos")]
    {
        let state = app.state::<AppState>();
        if state.call.lock().unwrap().joined || crate::local_host::installing(&app) ||
            app.try_state::<crate::engine_ipc::EngineState>().is_some_and(|engine| engine.jobs.busy()) ||
            state.resetting.swap(true, Ordering::SeqCst) {
            return Err(refused("reset.busy"));
        }
        let app_for_work = app.clone();
        let answer = tauri::async_runtime::spawn_blocking(move || begin_reset(&app_for_work)).await
            .map_err(|_| refused("reset.failed"))?;
        if answer.is_err() { state.resetting.store(false, Ordering::SeqCst); }
        answer
    }
}

#[cfg(target_os = "macos")]
fn begin_reset(app: &AppHandle) -> Answer {
    use sidevoice_local_host::{cli::Cli, paths::DataDirs};
    let dirs = DataDirs::from_env().ok_or_else(|| refused("reset.unavailable"))?;
    let service = match Cli::installed(&dirs) {
        Ok(cli) => cli.run(&["uninstall", "--json"], Duration::from_secs(180)).map_err(|_| refused("reset.service-failed"))?,
        Err(_) if !dirs.install_record().exists() => {
            // The local host must also agree that there is no managed installation.
            if !crate::local_host::absent(app) { return Err(refused("reset.service-failed")); }
            json!({"state": "absent", "agents": {}})
        }
        Err(_) => return Err(refused("reset.unsafe")),
    };
    let mut failed = Vec::<Value>::new();
    for item in ["openai", "elevenlabs"] {
        if crate::keychain::write(item, None).is_err() { failed.push(json!({"step": "keychain", "key": "reset.keychain-failed"})); }
    }
    let exe = std::env::current_exe().map_err(|_| refused("reset.unavailable"))?;
    let child = Command::new(exe).arg("--sidevoice-reset-helper")
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().map_err(|_| refused("reset.helper-failed"))?;
    *app.state::<AppState>().reset_helper.lock().unwrap() = Some(child);
    let app_to_quit = app.clone();
    std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(700)); app_to_quit.exit(0); });
    let agents = service.get("agents").cloned().unwrap_or_else(|| json!({}));
    Ok(json!({"state": if failed.is_empty() { "needs-restart" } else { "partial" },
        "service": if service.get("state").and_then(Value::as_str) == Some("absent") { "absent" } else { "removed" },
        "agents": agents, "failed": failed, "pending": ["app-data", "models", "webview", "cache", "saved-state", "logs", "preferences"]}))
}

#[cfg(target_os = "macos")]
fn report_path() -> Option<PathBuf> {
    Some(std::env::temp_dir().join(format!("dev.sidevoice.desktop-reset-{}.json", unsafe { libc::getuid() })))
}

#[cfg(target_os = "macos")]
fn remove_known(path: &Path, home: &Path) -> Result<(), &'static str> {
    let relative = path.strip_prefix(home).map_err(|_| "outside-home")?;
    if relative.as_os_str().is_empty() { return Err("home-root") }
    let mut current = home.to_path_buf();
    for piece in relative.components() {
        current.push(piece);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => return Err("symlink"),
            Ok(_) => {},
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err("metadata"),
        }
    }
    if !fs::symlink_metadata(path).map_err(|_| "metadata")?.is_dir() { return Err("not-directory") }
    fs::remove_dir_all(path).map_err(|_| "remove")
}

/// Called in the helper process, after its parent closes the pipe on exit.
pub fn run_reset_helper() {
    #[cfg(target_os = "macos")]
    {
        // A direct invocation without the pipe cannot race a running app: wait for EOF.
        let mut drain = Vec::new();
        if std::io::stdin().read_to_end(&mut drain).is_err() { return; }
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return };
        let Some(report) = report_path() else { return };
        let mut completed = Vec::new();
        let mut failed = Vec::new();
        let paths = [
            ("app-data", home.join("Library/Application Support/dev.sidevoice.desktop")),
            ("models", home.join("Library/Application Support/Sidevoice/Models")),
            ("webview", home.join("Library/WebKit/dev.sidevoice.desktop")),
            ("cache", home.join("Library/Caches/dev.sidevoice.desktop")),
            ("saved-state", home.join("Library/Saved Application State/dev.sidevoice.desktop.savedState")),
            ("logs", home.join("Library/Logs/dev.sidevoice.desktop")),
        ];
        for (name, path) in paths {
            match remove_known(&path, &home) {
                Ok(()) => completed.push(name),
                Err(key) => failed.push(json!({"step": name, "key": key})),
            }
        }
        let defaults = Command::new("/usr/bin/defaults").args(["delete", "dev.sidevoice.desktop"]).status();
        if defaults.is_ok_and(|s| s.success()) { completed.push("preferences"); }
        // Missing preferences are harmless: defaults exits nonzero when the domain is absent.
        let value = json!({"state": if failed.is_empty() { "complete" } else { "partial" }, "completed": completed, "failed": failed});
        if let Ok(bytes) = serde_json::to_vec(&value) {
            let tmp = report.with_extension(format!("json.tmp-{}", std::process::id()));
            let _ = fs::write(&tmp, bytes).and_then(|_| fs::rename(&tmp, &report));
        }
        if let Ok(exe) = std::env::current_exe() { let _ = Command::new(exe).spawn(); }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    #[test]
    fn cleanup_refuses_symlinks_and_home_itself() {
        let root = std::env::temp_dir().join(format!("sidevoice-reset-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("Library/Caches")).unwrap();
        let outside = root.with_extension("outside");
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("Library/Caches/dev.sidevoice.desktop")).unwrap();
        assert_eq!(remove_known(&root, &root), Err("home-root"));
        assert_eq!(remove_known(&root.join("Library/Caches/dev.sidevoice.desktop"), &root), Err("symlink"));
        assert!(outside.exists());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }
}
