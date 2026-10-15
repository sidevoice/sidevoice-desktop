//! The app's private pairing file, the machine's shared model root, and the confirmed reset.
//! Only the current room page can ask; no bridge method accepts a filesystem path.

use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
#[cfg(target_os = "macos")]
use std::sync::atomic::Ordering;
#[cfg(target_os = "macos")]
use std::time::Duration;
use tauri::{AppHandle, Manager, Webview};

#[cfg(target_os = "macos")]
use crate::AppState;

type Answer = Result<Value, Value>;

fn refused(key: &str) -> Value {
    json!({ "key": key })
}

#[cfg(target_os = "macos")]
fn retained_agents(agents: &Value) -> Vec<String> {
    agents
        .as_object()
        .map(|rows| {
            rows.iter()
                .filter_map(|(name, value)| {
                    let outcome = value.as_str().or_else(|| value.get("outcome").and_then(Value::as_str));
                    (!matches!(outcome, Some("removed" | "absent"))).then(|| name.clone())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn caller(app: &AppHandle, webview: &Webview) -> Result<(), Value> {
    crate::room_page(app, webview).map_err(|_| refused("reset.unauthorized"))
}

pub(crate) fn desktop_dir(app: &AppHandle) -> Result<PathBuf, Value> {
    app.path().app_data_dir().map_err(|_| refused("reset.unavailable"))
}

pub fn shared_model_root() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        home.is_absolute().then(|| home.join("Library/Application Support/Sidevoice/Models"))
    }
    #[cfg(target_os = "linux")]
    {
        let base =
            std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|path| path.is_absolute()).or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".local/share"))
            })?;
        Some(base.join("sidevoice-shared/models"))
    }
    #[cfg(target_os = "windows")]
    {
        let base = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
        base.is_absolute().then(|| base.join("Sidevoice/Models"))
    }
}

fn pairings_file(app: &AppHandle) -> Result<PathBuf, Value> {
    Ok(desktop_dir(app)?.join("pairings.json"))
}

fn pairings_path_is_safe(path: &Path) -> bool {
    path.ancestors().filter(|part| part.parent().is_some()).all(|part| match fs::symlink_metadata(part) {
        Ok(meta) => !meta.file_type().is_symlink(),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    })
}

fn checked_pairings(value: &Value) -> bool {
    let Some(rows) = value.get("pairings").and_then(Value::as_array) else { return false };
    let pending = value.get("pendingRevokes").and_then(Value::as_array);
    if rows.len() > 32
        || pending.is_some_and(|rows| rows.len() > 32)
        || !serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() <= 64 * 1024)
    {
        return false;
    }
    let valid = |row: &Value| {
        row.get("local").is_none()
            && ["fp", "public_key", "device_id", "token"]
                .iter()
                .all(|field| row.get(*field).and_then(Value::as_str).is_some_and(|s| !s.is_empty() && s.len() <= 4096))
            && row.get("urls").and_then(Value::as_array).is_some_and(|urls| {
                urls.len() <= 16 && urls.iter().all(|url| url.as_str().is_some_and(|s| s.len() <= 2048))
            })
    };
    rows.iter().all(valid) && pending.is_none_or(|rows| rows.iter().all(valid))
}

fn read_pairings(path: &Path) -> Answer {
    if !pairings_path_is_safe(path) {
        return Err(refused("reset.unsafe"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    match options.open(path).and_then(|file| {
        let mut bytes = Vec::new();
        file.take(64 * 1024 + 1).read_to_end(&mut bytes)?;
        Ok(bytes)
    }) {
        Ok(bytes) if bytes.len() <= 64 * 1024 => serde_json::from_slice::<Value>(&bytes)
            .ok()
            .filter(checked_pairings)
            .ok_or_else(|| refused("data.pairings-invalid")),
        Ok(_) => Err(refused("data.pairings-invalid")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({"pairings": [], "pendingRevokes": []})),
        Err(_) => Err(refused("data.pairings-read")),
    }
}

fn write_pairings(path: &Path, value: &Value) -> Answer {
    if !checked_pairings(value) {
        return Err(refused("data.pairings-invalid"));
    }
    if !pairings_path_is_safe(path) {
        return Err(refused("reset.unsafe"));
    }
    let parent = path.parent().ok_or_else(|| refused("data.pairings-write"))?;
    fs::create_dir_all(parent).map_err(|_| refused("data.pairings-write"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let meta = fs::symlink_metadata(parent).map_err(|_| refused("data.pairings-write"))?;
        if meta.uid() != unsafe { libc::geteuid() } || !meta.is_dir() {
            return Err(refused("reset.unsafe"));
        }
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|_| refused("data.pairings-write"))?;
    }
    if !pairings_path_is_safe(path) {
        return Err(refused("reset.unsafe"));
    }
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp).map_err(|_| refused("data.pairings-write"))?;
    let result = (|| {
        file.write_all(&serde_json::to_vec(value).map_err(|_| refused("data.pairings-invalid"))?)
            .map_err(|_| refused("data.pairings-write"))?;
        file.sync_all().map_err(|_| refused("data.pairings-write"))?;
        fs::rename(&tmp, path).map_err(|_| refused("data.pairings-write"))?;
        Ok(Value::Null)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
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
    #[cfg(unix)]
    {
        let path = report_path().ok_or_else(|| refused("reset.unavailable"))?;
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(refused("reset.unsafe"));
        }
        match fs::read(&path) {
            Ok(bytes) if bytes.len() <= 16 * 1024 => {
                let value = serde_json::from_slice(&bytes).map_err(|_| refused("reset.failed"))?;
                fs::remove_file(&path).map_err(|_| refused("reset.failed"))?;
                if let Some(dir) = path.parent() {
                    let _ = fs::remove_dir(dir);
                }
                Ok(value)
            }
            Ok(_) => Err(refused("reset.failed")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
            Err(_) => Err(refused("reset.failed")),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(refused("reset.unavailable"))
    }
}

#[tauri::command]
pub async fn data_delete_all(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err(refused("reset.unavailable"))
    }
    #[cfg(target_os = "macos")]
    {
        let state = app.state::<AppState>();
        if state.resetting.swap(true, Ordering::SeqCst) {
            return Err(refused("reset.busy"));
        }
        if state.call.lock().unwrap().joined
            || crate::local_host::installing(&app)
            || crate::updater::installing(&app)
            || app.try_state::<crate::engine_ipc::EngineState>().is_some_and(|engine| engine.jobs.busy())
        {
            state.resetting.store(false, Ordering::SeqCst);
            return Err(refused("reset.busy"));
        }
        if let Err(refusal) = require_confirmation(native_reset_confirmation(&app).await) {
            state.resetting.store(false, Ordering::SeqCst);
            return Err(refusal);
        }
        let app_for_work = app.clone();
        let answer = tauri::async_runtime::spawn_blocking(move || begin_reset(&app_for_work))
            .await
            .unwrap_or_else(|_| Err(refused("reset.failed")));
        if answer.is_err() {
            state.resetting.store(false, Ordering::SeqCst);
        }
        answer
    }
}

#[cfg(target_os = "macos")]
fn require_confirmation(accepted: bool) -> Result<(), Value> {
    accepted.then_some(()).ok_or_else(|| refused("reset.cancelled"))
}

#[cfg(target_os = "macos")]
async fn native_reset_confirmation(app: &AppHandle) -> bool {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    if app.run_on_main_thread(move || {
        use objc2::{class, msg_send, runtime::AnyObject};
        use objc2_foundation::NSString;
        // NSAlert's first button is the default. The destructive button is deliberately second.
        let confirmed = unsafe {
            let alert: *mut AnyObject = msg_send![class!(NSAlert), new];
            if alert.is_null() { false } else {
                let title = NSString::from_str("Delete all Sidevoice data?");
                let detail = NSString::from_str("This uninstalls the connector and deletes local Sidevoice data, models and saved keys. Remote machines and backups remain. This cannot be undone.");
                let cancel = NSString::from_str("Cancel");
                let delete = NSString::from_str("Delete all Sidevoice data");
                let _: () = msg_send![alert, setMessageText: &*title];
                let _: () = msg_send![alert, setInformativeText: &*detail];
                let _: *mut AnyObject = msg_send![alert, addButtonWithTitle: &*cancel];
                let _: *mut AnyObject = msg_send![alert, addButtonWithTitle: &*delete];
                let answer: isize = msg_send![alert, runModal];
                let _: () = msg_send![alert, release];
                answer == 1001 // NSAlertSecondButtonReturn
            }
        };
        let _ = sender.send(confirmed);
    }).is_err() { return false }
    tauri::async_runtime::spawn_blocking(move || receiver.recv().unwrap_or(false)).await.unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn begin_reset(app: &AppHandle) -> Answer {
    use sidevoice_local_host::{cli::Cli, paths::DataDirs};
    let dirs = DataDirs::from_env().ok_or_else(|| refused("reset.unavailable"))?;
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| refused("reset.unavailable"))?;
    validate_uninstall_scope(&home, &dirs.data).map_err(|reason| json!({"key": "reset.unsafe", "reason": reason}))?;
    let (service, service_outcome) = match Cli::installed(&dirs) {
        Ok(cli) => (
            {
                run_checked_uninstall(&home, &dirs.data, || {
                    cli.run(&["uninstall", "--json"], Duration::from_secs(180))
                        .map_err(|_| refused("reset.service-failed"))
                })?
            },
            "removed",
        ),
        Err(_) if !dirs.install_record().exists() => {
            // The observer's cached "absent" is not evidence: ask launchd and the process table now.
            if !fresh_service_absent(&home) {
                return Err(refused("reset.service-failed"));
            }
            (json!({"state": "absent", "agents": {}}), "absent")
        }
        Err(_) => return Err(refused("reset.unsafe")),
    };
    let mut failed = Vec::<Value>::new();
    if crate::keychain::delete_service().is_err() {
        failed.push(json!({"step": "keychain", "key": "reset.keychain-failed"}));
    }
    let exe = std::env::current_exe().map_err(|_| refused("reset.unavailable"))?;
    let mut child = Command::new(exe)
        .arg("--sidevoice-reset-helper")
        .arg(std::process::id().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| refused("reset.helper-failed"))?;
    let agents = service.get("agents").cloned().unwrap_or_else(|| json!({}));
    for agent in retained_agents(&agents) {
        failed.push(json!({"step": "agents", "key": "reset.agent-remains", "agent": agent}));
    }
    let initial = json!({"failed": failed, "agents": agents});
    let payload = serde_json::to_vec(&initial).map_err(|_| refused("reset.helper-failed"))?;
    if payload.len() > 16 * 1024 || child.stdin.as_mut().is_none_or(|stdin| stdin.write_all(&payload).is_err()) {
        let _ = child.kill();
        return Err(refused("reset.helper-failed"));
    }
    *app.state::<AppState>().reset_helper.lock().unwrap() = Some(child);
    let app_to_quit = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(700));
        app_to_quit.exit(0);
    });
    Ok(json!({"state": if failed.is_empty() { "needs-restart" } else { "partial" },
        "service": service_outcome,
        "agents": agents, "failed": failed, "pending": ["app-data", "models", "webview", "cache", "saved-state", "logs", "preferences"]}))
}

#[cfg(target_os = "macos")]
fn fresh_service_absent(home: &Path) -> bool {
    fresh_service_absent_with(
        home,
        |label| {
            let target = format!("gui/{}/{label}", unsafe { libc::geteuid() });
            let Ok(output) = Command::new("/bin/launchctl").args(["print", &target]).output() else { return false };
            if output.status.success() {
                return false;
            }
            let detail = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
            output.status.code() == Some(113) || detail.contains("could not find service")
        },
        || {
            let Ok(output) = Command::new("/bin/ps").args(["-axo", "comm="]).output() else { return false };
            output.status.success()
                && !String::from_utf8_lossy(&output.stdout).lines().any(|line| {
                    matches!(
                        Path::new(line.trim()).file_name().and_then(|name| name.to_str()),
                        Some("sidevoice-core" | "sidevoice-connector")
                    )
                })
        },
    )
}

#[cfg(target_os = "macos")]
fn fresh_service_absent_with(
    home: &Path,
    launchd_absent: impl Fn(&str) -> bool,
    processes_absent: impl Fn() -> bool,
) -> bool {
    if fs::canonicalize(home).ok().as_deref() != Some(home) {
        return false;
    }
    for label in ["dev.sidevoice.core", "dev.sidevoice.connector"] {
        if fs::symlink_metadata(home.join("Library/LaunchAgents").join(format!("{label}.plist"))).is_ok() {
            return false;
        }
        if !launchd_absent(label) {
            return false;
        }
    }
    processes_absent()
}

#[cfg(target_os = "macos")]
fn run_checked_uninstall<T>(home: &Path, data: &Path, invoke: impl FnOnce() -> Result<T, Value>) -> Result<T, Value> {
    validate_uninstall_scope(home, data).map_err(|reason| json!({"key": "reset.unsafe", "reason": reason}))?;
    invoke()
}

#[cfg(target_os = "macos")]
fn validate_uninstall_scope(home: &Path, data: &Path) -> Result<PathBuf, &'static str> {
    let data_home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    validate_uninstall_scope_at(home, data, data_home.as_deref())
}

#[cfg(target_os = "macos")]
fn validate_uninstall_scope_at(home: &Path, data: &Path, data_home: Option<&Path>) -> Result<PathBuf, &'static str> {
    if !clean_absolute(home) || fs::canonicalize(home).ok().as_deref() != Some(home) {
        return Err("unsafe-home");
    }
    let expected_data = home.join(".sidevoice");
    if data != expected_data || !clean_absolute(data) {
        return Err("outside-known-dirs");
    }
    let expected_releases = home.join(".local/share/sidevoice");
    if data_home.is_some_and(|path| path != home.join(".local/share")) {
        return Err("outside-known-dirs");
    }
    let record = data.join("install.json");
    if let Ok(meta) = fs::symlink_metadata(&record) {
        if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 64 * 1024 {
            return Err("unsafe-record");
        }
        let bytes = fs::read(&record).map_err(|_| "unsafe-record")?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| "unsafe-record")?;
        let releases = value.get("releases").and_then(Value::as_str).ok_or("unsafe-record")?;
        if Path::new(releases) != expected_releases || !clean_absolute(Path::new(releases)) {
            return Err("outside-known-dirs");
        }
    } else if fs::symlink_metadata(&record).is_err_and(|error| error.kind() != std::io::ErrorKind::NotFound) {
        return Err("unsafe-record");
    }
    guarded_tree(data, home, false, true, &mount_identity)?;
    guarded_tree(&expected_releases, home, false, true, &mount_identity)?;
    Ok(expected_releases)
}

#[cfg(target_os = "macos")]
fn report_path() -> Option<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let dir = std::env::temp_dir().join(format!("dev.sidevoice.desktop-reset-{}", unsafe { libc::getuid() }));
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    if builder.create(&dir).is_err_and(|e| e.kind() != std::io::ErrorKind::AlreadyExists) {
        return None;
    }
    let meta = fs::symlink_metadata(&dir).ok()?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != unsafe { libc::getuid() }
        || meta.mode() & 0o077 != 0
    {
        return None;
    }
    Some(dir.join("report.json"))
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct MountIdentity {
    device: u64,
    mounted_at: Vec<u8>,
}

#[cfg(target_os = "macos")]
fn mount_identity(fd: i32, _: &Path) -> Result<MountIdentity, &'static str> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let mut fs_stat: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 || unsafe { libc::fstatfs(fd, &mut fs_stat) } != 0 {
        return Err("inspect-mount");
    }
    let mounted_at = unsafe { std::ffi::CStr::from_ptr(fs_stat.f_mntonname.as_ptr()) }.to_bytes().to_vec();
    Ok(MountIdentity { device: stat.st_dev as u64, mounted_at })
}

#[cfg(target_os = "macos")]
fn clean_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|part| matches!(part, std::path::Component::RootDir | std::path::Component::Normal(_)))
}

#[cfg(target_os = "macos")]
fn known_cleanup_path(path: &Path, home: &Path) -> Result<(), &'static str> {
    if path == home {
        return Err("home-root");
    }
    if !clean_absolute(home) || !clean_absolute(path) || home.parent().is_none_or(|p| p == Path::new("/")) {
        return Err("unsafe-path");
    }
    let allowed = [
        home.join("Library/Application Support/dev.sidevoice.desktop"),
        home.join("Library/Application Support/Sidevoice/Models"),
        home.join("Library/WebKit/dev.sidevoice.desktop"),
        home.join("Library/Caches/dev.sidevoice.desktop"),
        home.join("Library/Saved Application State/dev.sidevoice.desktop.savedState"),
        home.join("Library/Logs/dev.sidevoice.desktop"),
    ];
    if !allowed.iter().any(|candidate| candidate == path) {
        return Err("outside-known-dirs");
    }
    if fs::canonicalize(home).ok().as_deref() != Some(home) {
        return Err("unsafe-home");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_dir_at(parent: i32, name: &std::ffi::OsStr) -> Result<std::os::fd::OwnedFd, &'static str> {
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(name.as_bytes()).map_err(|_| "unsafe-name")?;
    let fd = unsafe {
        libc::openat(parent, name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
    };
    if fd < 0 {
        return Err("unsafe-directory");
    }
    Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) })
}

#[cfg(target_os = "macos")]
fn open_absolute_dir(path: &Path) -> Result<std::os::fd::OwnedFd, &'static str> {
    use std::os::fd::{AsRawFd, FromRawFd};
    if !clean_absolute(path) {
        return Err("unsafe-path");
    }
    let root = unsafe { libc::open(c"/".as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if root < 0 {
        return Err("unsafe-path");
    }
    let mut current = unsafe { std::os::fd::OwnedFd::from_raw_fd(root) };
    for part in path.components().skip(1) {
        let std::path::Component::Normal(name) = part else { return Err("unsafe-path") };
        current = open_dir_at(current.as_raw_fd(), name)?;
    }
    Ok(current)
}

#[cfg(target_os = "macos")]
fn same_entry(parent: i32, name: &std::ffi::CStr, opened: i32) -> Result<(), &'static str> {
    let mut named: libc::stat = unsafe { std::mem::zeroed() };
    let mut held: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatat(parent, name.as_ptr(), &mut named, libc::AT_SYMLINK_NOFOLLOW) } != 0
        || unsafe { libc::fstat(opened, &mut held) } != 0
        || named.st_dev != held.st_dev
        || named.st_ino != held.st_ino
    {
        return Err("changed-path");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn walk_owned(
    fd: i32,
    relative: &Path,
    expected: &MountIdentity,
    probe: &impl Fn(i32, &Path) -> Result<MountIdentity, &'static str>,
    remove: bool,
    allow_links: bool,
    depth: usize,
) -> Result<(), &'static str> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    if depth > 64 {
        return Err("too-deep");
    }
    if probe(fd, relative)? != *expected {
        return Err("mount-boundary");
    }
    let copy = unsafe { libc::dup(fd) };
    if copy < 0 {
        return Err("inspect");
    }
    let stream = unsafe { libc::fdopendir(copy) };
    if stream.is_null() {
        unsafe { libc::close(copy) };
        return Err("inspect");
    }
    let result = (|| {
        loop {
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                break;
            }
            let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            let child_relative = relative.join(std::ffi::OsStr::from_bytes(name.to_bytes()));
            let mut stat: libc::stat = unsafe { std::mem::zeroed() };
            if unsafe { libc::fstatat(fd, name.as_ptr(), &mut stat, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
                return Err("inspect");
            }
            if stat.st_dev as u64 != expected.device {
                return Err("mount-boundary");
            }
            let kind = stat.st_mode & libc::S_IFMT;
            if kind == libc::S_IFLNK {
                if allow_links {
                    continue;
                } else {
                    return Err("symlink");
                }
            }
            if kind == libc::S_IFDIR {
                let child = open_dir_at(fd, std::ffi::OsStr::from_bytes(name.to_bytes()))?;
                walk_owned(child.as_raw_fd(), &child_relative, expected, probe, remove, allow_links, depth + 1)?;
                if remove {
                    same_entry(fd, name, child.as_raw_fd())?;
                    if unsafe { libc::unlinkat(fd, name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
                        return Err("remove");
                    }
                }
            } else if kind == libc::S_IFREG {
                let file = unsafe {
                    libc::openat(
                        fd,
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                    )
                };
                if file < 0 {
                    return Err("unsafe-file");
                }
                let file = unsafe { std::os::fd::OwnedFd::from_raw_fd(file) };
                if probe(file.as_raw_fd(), &child_relative)? != *expected {
                    return Err("mount-boundary");
                }
                if remove {
                    same_entry(fd, name, file.as_raw_fd())?;
                    if unsafe { libc::unlinkat(fd, name.as_ptr(), 0) } != 0 {
                        return Err("remove");
                    }
                }
            } else if !allow_links {
                return Err("unsafe-entry");
            }
        }
        Ok(())
    })();
    unsafe { libc::closedir(stream) };
    result
}

#[cfg(target_os = "macos")]
fn guarded_tree(
    path: &Path,
    home: &Path,
    remove: bool,
    allow_links: bool,
    probe: &impl Fn(i32, &Path) -> Result<MountIdentity, &'static str>,
) -> Result<(), &'static str> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    let relative = path.strip_prefix(home).map_err(|_| "outside-home")?;
    if relative.as_os_str().is_empty() {
        return Err("home-root");
    }
    if !clean_absolute(path) || !clean_absolute(home) {
        return Err("unsafe-path");
    }
    if fs::canonicalize(home).ok().as_deref() != Some(home) {
        return Err("unsafe-home");
    }
    let home_fd = open_absolute_dir(home)?;
    let expected = probe(home_fd.as_raw_fd(), Path::new(""))?;
    if expected.mounted_at == home.as_os_str().as_bytes() {
        return Err("volume-root");
    }
    let parts: Vec<_> = relative.components().collect();
    let mut parent = home_fd;
    let mut prefix = PathBuf::new();
    for part in &parts[..parts.len() - 1] {
        let std::path::Component::Normal(name) = part else { return Err("unsafe-path") };
        prefix.push(name);
        parent = match open_dir_at(parent.as_raw_fd(), name) {
            Ok(fd) => fd,
            Err(_) if !parent_path_exists(parent.as_raw_fd(), name) => return Ok(()),
            Err(error) => return Err(error),
        };
        if probe(parent.as_raw_fd(), &prefix)? != expected {
            return Err("mount-boundary");
        }
    }
    let std::path::Component::Normal(name) = parts[parts.len() - 1] else { return Err("unsafe-path") };
    let target = match open_dir_at(parent.as_raw_fd(), name) {
        Ok(fd) => fd,
        Err(_) if !parent_path_exists(parent.as_raw_fd(), name) => return Ok(()),
        Err(error) => return Err(error),
    };
    walk_owned(target.as_raw_fd(), relative, &expected, probe, false, allow_links, 0)?;
    if remove {
        walk_owned(target.as_raw_fd(), relative, &expected, probe, true, allow_links, 0)?;
        let name = std::ffi::CString::new(name.as_bytes()).map_err(|_| "unsafe-name")?;
        same_entry(parent.as_raw_fd(), &name, target.as_raw_fd())?;
        if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
            return Err("remove");
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn remove_known_with(
    path: &Path,
    home: &Path,
    probe: &impl Fn(i32, &Path) -> Result<MountIdentity, &'static str>,
) -> Result<(), &'static str> {
    known_cleanup_path(path, home)?;
    guarded_tree(path, home, true, false, probe)
}

#[cfg(target_os = "macos")]
fn remove_known(path: &Path, home: &Path) -> Result<(), &'static str> {
    remove_known_with(path, home, &mount_identity)
}

#[cfg(target_os = "macos")]
fn parent_path_exists(parent: i32, name: &std::ffi::OsStr) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(name) = std::ffi::CString::new(name.as_bytes()) else { return true };
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::fstatat(parent, name.as_ptr(), &mut stat, libc::AT_SYMLINK_NOFOLLOW) };
    result == 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::NotFound
}

#[cfg(target_os = "macos")]
fn webkit_children(parent: u32) -> Option<Vec<u32>> {
    let output = Command::new("/bin/ps").args(["-axo", "pid=,ppid=,comm="]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let rows: Vec<(u32, u32, String)> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.collect::<Vec<_>>().join(" ")))
        })
        .collect();
    let mut descendants = vec![parent];
    for _ in 0..8 {
        let children: Vec<_> = rows
            .iter()
            .filter(|(_, ppid, _)| descendants.contains(ppid))
            .map(|(pid, _, _)| *pid)
            .filter(|pid| !descendants.contains(pid))
            .collect();
        if children.is_empty() {
            break;
        }
        descendants.extend(children);
    }
    Some(
        rows.iter()
            .filter(|(pid, _, command)| descendants.contains(pid) && command.contains("WebKit"))
            .map(|(pid, _, _)| *pid)
            .collect(),
    )
}

#[cfg(target_os = "macos")]
fn webkit_gone(pids: &[u32]) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if pids.iter().all(|pid| unsafe { libc::kill(*pid as i32, 0) } != 0 &&
            std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)) { return true }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Called in the helper process, after its parent closes the pipe on exit.
pub fn run_reset_helper(parent_pid: Option<u32>) {
    #[cfg(target_os = "macos")]
    {
        let Some(parent_pid) = parent_pid else { return };
        if unsafe { libc::getppid() as u32 } != parent_pid {
            return;
        }
        let webkit = webkit_children(parent_pid);
        // The parent writes the pre-exit failures then keeps the pipe open until it exits.
        let mut drain = Vec::new();
        if std::io::stdin().read_to_end(&mut drain).is_err() {
            return;
        }
        if drain.is_empty() || drain.len() > 16 * 1024 {
            return;
        }
        let Ok(initial) = serde_json::from_slice::<Value>(&drain) else { return };
        let Some(initial_failed) = initial.get("failed").and_then(Value::as_array) else { return };
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return };
        let Some(report) = report_path() else { return };
        let mut completed = Vec::new();
        let mut failed = initial_failed.clone();
        let webkit_safe = webkit.as_ref().is_some_and(|children| webkit_gone(children));
        let paths = [
            ("app-data", home.join("Library/Application Support/dev.sidevoice.desktop")),
            ("models", home.join("Library/Application Support/Sidevoice/Models")),
            ("webview", home.join("Library/WebKit/dev.sidevoice.desktop")),
            ("cache", home.join("Library/Caches/dev.sidevoice.desktop")),
            ("saved-state", home.join("Library/Saved Application State/dev.sidevoice.desktop.savedState")),
            ("logs", home.join("Library/Logs/dev.sidevoice.desktop")),
        ];
        for (name, path) in paths {
            if name == "webview" && !webkit_safe {
                failed.push(json!({"step": "webview", "key": "reset.process-busy"}));
                continue;
            }
            match remove_known(&path, &home) {
                Ok(()) => completed.push(name),
                Err(key) => failed.push(json!({"step": name, "key": key})),
            }
        }
        let domains = Command::new("/usr/bin/defaults").arg("domains").output();
        match domains {
            Ok(output) if output.status.success() => {
                let present =
                    String::from_utf8_lossy(&output.stdout).split(',').any(|d| d.trim() == "dev.sidevoice.desktop");
                if !present
                    || Command::new("/usr/bin/defaults")
                        .args(["delete", "dev.sidevoice.desktop"])
                        .status()
                        .is_ok_and(|s| s.success())
                {
                    completed.push("preferences");
                } else {
                    failed.push(json!({"step": "preferences", "key": "remove"}));
                }
            }
            _ => failed.push(json!({"step": "preferences", "key": "inspect"})),
        }
        let value = json!({"state": if failed.is_empty() { "complete" } else { "partial" }, "completed": completed, "failed": failed,
            "agents": initial.get("agents").cloned().unwrap_or_else(|| json!({}))});
        if let Ok(bytes) = serde_json::to_vec(&value) {
            let tmp = report.with_extension(format!("json.tmp-{}", std::process::id()));
            use std::os::unix::fs::OpenOptionsExt;
            let _ = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)
                .and_then(|mut file| {
                    file.write_all(&bytes)?;
                    file.sync_all()
                })
                .and_then(|_| fs::rename(&tmp, &report));
        }
        if let Ok(exe) = std::env::current_exe() {
            let _ = Command::new(exe).spawn();
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = parent_pid;
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    #[test]
    fn cleanup_refuses_symlinked_target_parent_and_child() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let canonical = fs::canonicalize(home.path()).unwrap();
        let root = canonical.as_path();
        fs::write(outside.path().join("sentinel"), "unrelated").unwrap();
        fs::create_dir_all(root.join("Library/Caches")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("Library/Caches/dev.sidevoice.desktop")).unwrap();
        assert_eq!(remove_known(root, root), Err("home-root"));
        assert!(remove_known(&root.join("Library/Caches/dev.sidevoice.desktop"), root).is_err());
        assert!(outside.path().exists());
        assert_eq!(fs::read(outside.path().join("sentinel")).unwrap(), b"unrelated");
        fs::remove_file(root.join("Library/Caches/dev.sidevoice.desktop")).unwrap();
        fs::remove_dir(root.join("Library/Caches")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("Library/Caches")).unwrap();
        assert!(remove_known(&root.join("Library/Caches/dev.sidevoice.desktop"), root).is_err());
        assert!(outside.path().exists());
        assert_eq!(fs::read(outside.path().join("sentinel")).unwrap(), b"unrelated");
        fs::remove_file(root.join("Library/Caches")).unwrap();
        fs::create_dir_all(root.join("Library/Caches/dev.sidevoice.desktop")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("Library/Caches/dev.sidevoice.desktop/escape")).unwrap();
        assert_eq!(remove_known(&root.join("Library/Caches/dev.sidevoice.desktop"), root), Err("symlink"));
        assert!(root.join("Library/Caches/dev.sidevoice.desktop").exists());
        assert!(outside.path().exists());
        assert_eq!(fs::read(outside.path().join("sentinel")).unwrap(), b"unrelated");
    }

    #[test]
    fn cleanup_handles_missing_and_only_removes_named_directory() {
        let home = tempfile::tempdir().unwrap();
        let canonical = fs::canonicalize(home.path()).unwrap();
        let root = canonical.as_path();
        let parent = root.join("Library/Caches");
        fs::create_dir_all(&parent).unwrap();
        let target = parent.join("dev.sidevoice.desktop");
        assert_eq!(remove_known(&target, root), Ok(()));
        fs::create_dir(&target).unwrap();
        fs::write(target.join("owned"), "only in temp").unwrap();
        fs::create_dir(parent.join("unrelated")).unwrap();
        assert_eq!(remove_known(&target, root), Ok(()));
        assert!(!target.exists());
        assert!(parent.join("unrelated").exists());
    }

    #[test]
    fn cleanup_refuses_target_and_child_mount_boundaries_before_erasing_sentinels() {
        let home = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(home.path()).unwrap();
        let target = home.join("Library/Caches/dev.sidevoice.desktop");
        fs::create_dir_all(target.join("mounted")).unwrap();
        fs::write(target.join("mounted/sentinel"), b"unrelated").unwrap();
        fs::write(target.join("owned"), b"owned").unwrap();
        let target_relative = Path::new("Library/Caches/dev.sidevoice.desktop");
        let target_mount = |fd, relative: &Path| {
            let mut id = mount_identity(fd, relative)?;
            if relative == target_relative {
                id.mounted_at = b"/foreign-volume".to_vec();
            }
            Ok(id)
        };
        assert_eq!(remove_known_with(&target, &home, &target_mount), Err("mount-boundary"));
        assert!(target.join("owned").exists());
        let child_mount = |fd, relative: &Path| {
            let mut id = mount_identity(fd, relative)?;
            if relative.ends_with("mounted") {
                id.device += 1;
            }
            Ok(id)
        };
        assert_eq!(remove_known_with(&target, &home, &child_mount), Err("mount-boundary"));
        assert_eq!(fs::read(target.join("mounted/sentinel")).unwrap(), b"unrelated");
        assert!(target.join("owned").exists());
    }

    #[test]
    fn connector_uninstall_scope_refuses_overrides_before_fake_cli_runs() {
        let home = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(home.path()).unwrap();
        let mut invoked = false;
        for data in
            [home.clone(), PathBuf::from("/"), home.join("../other"), home.join("Documents"), PathBuf::from("relative")]
        {
            assert!(run_checked_uninstall(&home, &data, || {
                invoked = true;
                Ok(())
            })
            .is_err());
        }
        assert!(!invoked);
        assert_eq!(
            validate_uninstall_scope_at(&home, &home.join(".sidevoice"), Some(&home.join("Documents"))),
            Err("outside-known-dirs")
        );
        let data = home.join(".sidevoice");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("install.json"), json!({"releases": home.join("Documents/sidevoice")}).to_string())
            .unwrap();
        assert_eq!(validate_uninstall_scope_at(&home, &data, None), Err("outside-known-dirs"));
        fs::write(data.join("install.json"), json!({"releases": home.join(".local/share/sidevoice")}).to_string())
            .unwrap();
        assert!(validate_uninstall_scope_at(&home, &data, None).is_ok());
    }

    #[test]
    fn retained_agent_outcomes_are_reported() {
        let agents = json!({"claude": "removed", "codex": {"outcome": "foreign"}, "cursor": {"outcome": "failed"}});
        assert_eq!(retained_agents(&agents), ["codex", "cursor"]);
    }

    #[test]
    fn direct_command_requires_native_confirmation() {
        assert_eq!(require_confirmation(false), Err(refused("reset.cancelled")));
        assert_eq!(require_confirmation(true), Ok(()));
    }

    #[test]
    fn missing_install_record_requires_fresh_launchd_and_process_absence() {
        let home = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(home.path()).unwrap();
        assert!(!fresh_service_absent_with(&home, |_| false, || true));
        assert!(!fresh_service_absent_with(&home, |_| true, || false));
        assert!(fresh_service_absent_with(&home, |_| true, || true));
        fs::create_dir_all(home.join("Library/LaunchAgents")).unwrap();
        fs::write(home.join("Library/LaunchAgents/dev.sidevoice.core.plist"), b"orphan").unwrap();
        assert!(!fresh_service_absent_with(&home, |_| true, || true));
    }

    #[test]
    fn pairings_refuse_symlinked_parent_and_file() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(home.path().join("app")).unwrap();
        std::os::unix::fs::symlink(outside.path(), home.path().join("app/redirect")).unwrap();
        let redirected = home.path().join("app/redirect/pairings.json");
        assert_eq!(read_pairings(&redirected), Err(refused("reset.unsafe")));
        assert_eq!(write_pairings(&redirected, &json!({"pairings": []})), Err(refused("reset.unsafe")));
        let file = home.path().join("app/pairings.json");
        std::os::unix::fs::symlink(outside.path().join("other.json"), &file).unwrap();
        assert_eq!(read_pairings(&file), Err(refused("reset.unsafe")));
        assert!(!outside.path().join("other.json").exists());
    }

    #[test]
    fn pending_revocations_share_the_private_native_pairings_file() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join("app/pairings.json");
        let pending = json!({"fp":"fp", "public_key":"pk", "device_id":"id", "token":"live-secret", "urls":["https://example.test"]});
        let value = json!({"pairings":[], "pendingRevokes":[pending]});
        assert_eq!(write_pairings(&file, &value), Ok(Value::Null));
        assert_eq!(read_pairings(&file), Ok(value));
        assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o077, 0);
        assert_eq!(fs::metadata(file.parent().unwrap()).unwrap().permissions().mode() & 0o077, 0);
    }
}
