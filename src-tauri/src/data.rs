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

use crate::AppState;

#[derive(Default)]
pub(crate) struct Admission {
    active: usize,
    resetting: bool,
}

impl Admission {
    fn enter(&mut self) -> bool {
        if self.resetting {
            return false;
        }
        self.active += 1;
        true
    }
    fn leave(&mut self) {
        self.active -= 1;
    }
    #[cfg(target_os = "macos")]
    fn begin_reset(&mut self) -> bool {
        if self.resetting || self.active != 0 {
            return false;
        }
        self.resetting = true;
        true
    }
    #[cfg(target_os = "macos")]
    fn end_reset(&mut self) {
        self.resetting = false;
    }
}

pub(crate) struct Permit(AppHandle);

impl Drop for Permit {
    fn drop(&mut self) {
        let state = self.0.state::<AppState>();
        let mut gate = state.reset_admission.lock().unwrap();
        gate.leave();
    }
}

pub(crate) fn admit(app: &AppHandle) -> Option<Permit> {
    let state = app.state::<AppState>();
    let mut gate = state.reset_admission.lock().unwrap();
    if !gate.enter() {
        return None;
    }
    Some(Permit(app.clone()))
}

#[cfg(target_os = "macos")]
fn begin_exclusive(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let mut gate = state.reset_admission.lock().unwrap();
    if !gate.begin_reset() {
        return false;
    }
    state.resetting.store(true, Ordering::SeqCst);
    true
}

#[cfg(target_os = "macos")]
fn end_exclusive(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut gate = state.reset_admission.lock().unwrap();
    gate.end_reset();
    state.resetting.store(false, Ordering::SeqCst);
}

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
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return false;
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if meta.file_attributes() & 0x400 != 0 {
                    return false;
                } // FILE_ATTRIBUTE_REPARSE_POINT, including junctions
            }
            true
        }
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
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
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
    #[cfg(unix)]
    {
        return write_pairings_unix(path, value, || {});
    }
    #[cfg(windows)]
    {
        return write_pairings_windows(path, value, || {});
    }
}

#[cfg(unix)]
fn pairing_parent_unix(parent: &Path) -> Result<std::os::fd::OwnedFd, Value> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    if !parent.is_absolute()
        || parent
            .components()
            .any(|part| !matches!(part, std::path::Component::RootDir | std::path::Component::Normal(_)))
    {
        return Err(refused("reset.unsafe"));
    }
    let root = unsafe { libc::open(c"/".as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if root < 0 {
        return Err(refused("reset.unsafe"));
    }
    let mut current = unsafe { std::os::fd::OwnedFd::from_raw_fd(root) };
    for component in parent.components().skip(1) {
        let std::path::Component::Normal(name) = component else { return Err(refused("reset.unsafe")) };
        let name = std::ffi::CString::new(name.as_bytes()).map_err(|_| refused("reset.unsafe"))?;
        let mut fd = unsafe {
            libc::openat(
                current.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            if unsafe { libc::mkdirat(current.as_raw_fd(), name.as_ptr(), 0o700) } != 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(refused("data.pairings-write"));
            }
            fd = unsafe {
                libc::openat(
                    current.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
        }
        if fd < 0 {
            return Err(refused("reset.unsafe"));
        }
        current = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
    }
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(current.as_raw_fd(), &mut stat) } != 0 || stat.st_uid != unsafe { libc::geteuid() } {
        return Err(refused("reset.unsafe"));
    }
    if unsafe { libc::fchmod(current.as_raw_fd(), 0o700) } != 0 {
        return Err(refused("data.pairings-write"));
    }
    Ok(current)
}

#[cfg(unix)]
fn write_pairings_unix(path: &Path, value: &Value, before_rename: impl FnOnce()) -> Answer {
    use std::os::fd::{AsRawFd, FromRawFd};
    let parent = path.parent().ok_or_else(|| refused("data.pairings-write"))?;
    let held = pairing_parent_unix(parent)?;
    let tmp = std::ffi::CString::new(format!("pairings.json.tmp-{}", std::process::id())).unwrap();
    let final_name = c"pairings.json";
    let fd = unsafe {
        libc::openat(
            held.as_raw_fd(),
            tmp.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(refused("data.pairings-write"));
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let result = (|| {
        file.write_all(&serde_json::to_vec(value).map_err(|_| refused("data.pairings-invalid"))?)
            .map_err(|_| refused("data.pairings-write"))?;
        file.sync_all().map_err(|_| refused("data.pairings-write"))?;
        before_rename();
        // The caller's pathname may have been replaced; the held directory can never follow its replacement.
        let reopened = pairing_parent_unix(parent)?;
        let mut original: libc::stat = unsafe { std::mem::zeroed() };
        let mut current: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(held.as_raw_fd(), &mut original) } != 0
            || unsafe { libc::fstat(reopened.as_raw_fd(), &mut current) } != 0
            || original.st_dev != current.st_dev
            || original.st_ino != current.st_ino
        {
            return Err(refused("reset.unsafe"));
        }
        if unsafe { libc::renameat(held.as_raw_fd(), tmp.as_ptr(), held.as_raw_fd(), final_name.as_ptr()) } != 0 {
            return Err(refused("data.pairings-write"));
        }
        file.sync_all().map_err(|_| refused("data.pairings-write"))?;
        Ok(Value::Null)
    })();
    if result.is_err() {
        unsafe { libc::unlinkat(held.as_raw_fd(), tmp.as_ptr(), 0) };
    }
    result
}

#[cfg(windows)]
fn pairing_parent_windows(parent: &Path) -> Result<Vec<std::fs::File>, Value> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    if !parent.is_absolute()
        || parent
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir | std::path::Component::CurDir))
    {
        return Err(refused("reset.unsafe"));
    }
    let mut held = Vec::new();
    let mut current = PathBuf::new();
    for part in parent.components() {
        if matches!(part, std::path::Component::Prefix(_)) {
            current.push(part.as_os_str());
            continue;
        }
        current.push(part.as_os_str());
        if !current.exists() {
            fs::create_dir(&current).map_err(|_| refused("data.pairings-write"))?;
        }
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE) // deny delete sharing: no ancestor rename/junction replacement
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&current)
            .map_err(|_| refused("reset.unsafe"))?;
        let meta = file.metadata().map_err(|_| refused("reset.unsafe"))?;
        if !meta.is_dir() || meta.file_attributes() & 0x400 != 0 {
            return Err(refused("reset.unsafe"));
        }
        held.push(file);
    }
    Ok(held)
}

#[cfg(windows)]
fn write_pairings_windows(path: &Path, value: &Value, before_rename: impl FnOnce()) -> Answer {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
    let parent = path.parent().ok_or_else(|| refused("data.pairings-write"))?;
    let _held = pairing_parent_windows(parent)?;
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&tmp)
        .map_err(|_| refused("data.pairings-write"))?;
    let result = (|| {
        file.write_all(&serde_json::to_vec(value).map_err(|_| refused("data.pairings-invalid"))?)
            .map_err(|_| refused("data.pairings-write"))?;
        file.sync_all().map_err(|_| refused("data.pairings-write"))?;
        before_rename();
        if !pairings_path_is_safe(path) {
            return Err(refused("reset.unsafe"));
        }
        fs::rename(&tmp, path).map_err(|_| refused("data.pairings-write"))?;
        Ok(Value::Null)
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
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
    #[cfg(target_os = "macos")]
    {
        let path = report_path().ok_or_else(|| refused("reset.unavailable"))?;
        consume_report_at(&path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(refused("reset.unavailable"))
    }
}

#[cfg(target_os = "macos")]
fn consume_report_at(path: &Path) -> Answer {
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(refused("reset.unsafe"));
    }
    match fs::read(path) {
        Ok(bytes) if bytes.len() <= 16 * 1024 => {
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| refused("reset.failed"))?;
            let journal = path.with_file_name("journal.json");
            if let Err(error) = fs::remove_file(&journal) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(refused("reset.report-failed"));
                }
            }
            fs::remove_file(path).map_err(|_| refused("reset.failed"))?;
            Ok(value)
        }
        Ok(_) => Err(refused("reset.failed")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let journal = path.with_file_name("journal.json");
            if fs::symlink_metadata(&journal).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(refused("reset.unsafe"));
            }
            match fs::read(&journal) {
                Ok(bytes) if bytes.len() <= 16 * 1024 => {
                    let progress: Value = serde_json::from_slice(&bytes).map_err(|_| refused("reset.report-failed"))?;
                    let mut failed = progress.get("failed").and_then(Value::as_array).cloned().unwrap_or_default();
                    failed.push(json!({"step":"helper", "key":"reset.interrupted"}));
                    fs::remove_file(&journal).map_err(|_| refused("reset.report-failed"))?;
                    Ok(
                        json!({"state":"partial", "completed":progress.get("completed").cloned().unwrap_or_else(|| json!([])), "failed":failed, "agents":{}}),
                    )
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
                _ => Err(refused("reset.report-failed")),
            }
        }
        Err(_) => Err(refused("reset.failed")),
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
        if !begin_exclusive(&app) {
            return Err(refused("reset.busy"));
        }
        if state.call.lock().unwrap().joined
            || crate::local_host::installing(&app)
            || crate::updater::installing(&app)
            || app.try_state::<crate::engine_ipc::EngineState>().is_some_and(|engine| engine.jobs.busy())
        {
            end_exclusive(&app);
            return Err(refused("reset.busy"));
        }
        if let Err(refusal) = require_confirmation(native_reset_confirmation(&app).await) {
            end_exclusive(&app);
            return Err(refusal);
        }
        let app_for_work = app.clone();
        let answer = tauri::async_runtime::spawn_blocking(move || begin_reset(&app_for_work))
            .await
            .unwrap_or_else(|_| Err(refused("reset.failed")));
        if answer.is_err() {
            end_exclusive(&app);
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
    let reset_lock = open_reset_lock().map_err(|_| refused("reset.helper-failed"))?;
    if !try_lock(&reset_lock) {
        return Err(refused("reset.busy"));
    }
    let dirs = DataDirs::from_env().ok_or_else(|| refused("reset.unavailable"))?;
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| refused("reset.unavailable"))?;
    validate_uninstall_scope(&home, &dirs.data).map_err(|reason| json!({"key": "reset.unsafe", "reason": reason}))?;
    let report = report_path().ok_or_else(|| refused("reset.helper-failed"))?;
    if let Err(error) = fs::remove_file(&report) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(refused("reset.report-failed"));
        }
    }
    let journal = report.with_file_name("journal.json");
    write_atomic_json(&journal, &json!({"stage":"preflight", "completed":[], "failed":[]}))
        .map_err(|_| refused("reset.helper-failed"))?;
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
            if !fresh_service_absent(&home) || residual_connector_data(&home, &dirs.data) {
                return Err(refused("reset.service-failed"));
            }
            (json!({"state": "absent", "agents": {}}), "absent")
        }
        Err(_) => return Err(refused("reset.unsafe")),
    };
    write_atomic_json(&journal, &json!({"stage":"service", "completed":["service"], "failed":[]}))
        .map_err(|_| refused("reset.report-failed"))?;
    let mut failed = Vec::<Value>::new();
    if crate::keychain::delete_service().is_err() {
        failed.push(json!({"step": "keychain", "key": "reset.keychain-failed"}));
    }
    write_atomic_json(
        &journal,
        &json!({"stage":"keychain", "completed":["service", "keychain"], "failed":failed.clone()}),
    )
    .map_err(|_| refused("reset.report-failed"))?;
    let agents = service.get("agents").cloned().unwrap_or_else(|| json!({}));
    for agent in retained_agents(&agents) {
        failed.push(json!({"step": "agents", "key": "reset.agent-remains", "agent": agent}));
    }
    write_atomic_json(
        &journal,
        &json!({"stage":"agent-results", "completed":["service", "keychain"], "failed":failed.clone()}),
    )
    .map_err(|_| refused("reset.report-failed"))?;
    let exe = std::env::current_exe().map_err(|_| refused("reset.unavailable"))?;
    let inherited_lock = inherit_lock(&reset_lock).map_err(|_| refused("reset.helper-failed"))?;
    let spawned = Command::new(exe)
        .arg("--sidevoice-reset-helper")
        .arg(std::process::id().to_string())
        .arg(inherited_lock.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::fcntl(reset_lock.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    let mut child = spawned.map_err(|_| refused("reset.helper-failed"))?;
    let initial = json!({"failed": failed, "agents": agents});
    let payload = serde_json::to_vec(&initial).map_err(|_| refused("reset.helper-failed"))?;
    if payload.len() > 16 * 1024 || child.stdin.as_mut().is_none_or(|stdin| stdin.write_all(&payload).is_err()) {
        let _ = child.kill();
        return Err(refused("reset.helper-failed"));
    }
    let mut ready = child.stdout.take().ok_or_else(|| refused("reset.helper-failed"))?;
    let (sent, received) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = [0u8; 6];
        let _ = sent.send(ready.read_exact(&mut bytes).is_ok() && &bytes == b"ready\n");
    });
    if received.recv_timeout(Duration::from_secs(5)) != Ok(true) {
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
fn residual_connector_data(home: &Path, data: &Path) -> bool {
    let releases = home.join(".local/share/sidevoice");
    // A missing record is not proof that an interrupted uninstall finished its data phase.
    if fs::symlink_metadata(releases).is_ok() {
        return true;
    }
    let entries = match fs::read_dir(data) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    for entry in entries {
        let Ok(entry) = entry else { return true };
        let name = entry.file_name();
        if !["install.lock", "connector.lock", "agents.lock", "node-stopped.json"].iter().any(|kept| name == *kept) {
            return true;
        }
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            return true;
        }
    }
    false
}

#[cfg(target_os = "macos")]
fn write_atomic_json(path: &Path, value: &Value) -> Result<(), &'static str> {
    use std::os::unix::fs::OpenOptionsExt;
    let bytes = serde_json::to_vec(value).map_err(|_| "report-write")?;
    if bytes.len() > 16 * 1024 {
        return Err("report-write");
    }
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)
            .map_err(|_| "report-write")?;
        file.write_all(&bytes).map_err(|_| "report-write")?;
        file.sync_all().map_err(|_| "report-write")?;
        fs::rename(&tmp, path).map_err(|_| "report-write")?;
        std::fs::File::open(path.parent().ok_or("report-write")?)
            .and_then(|dir| dir.sync_all())
            .map_err(|_| "report-write")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
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
            output.status.success() && !managed_process_in_ps(&output.stdout)
        },
    )
}

#[cfg(target_os = "macos")]
fn managed_process_in_ps(output: &[u8]) -> bool {
    String::from_utf8_lossy(output).lines().any(|line| {
        let path = Path::new(line.trim());
        matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("sidevoice-core-rust" | "sidevoice-core" | "sidevoice-connector")
        )
    })
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
        match fs::symlink_metadata(home.join("Library/LaunchAgents").join(format!("{label}.plist"))) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return false,
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
    if account_home().as_deref() != Some(home) {
        return Err("unsafe-home");
    }
    let data_home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    validate_uninstall_scope_at(home, data, data_home.as_deref())
}

#[cfg(target_os = "macos")]
fn account_home() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    let mut buffer = vec![0u8; 16 * 1024];
    let result =
        unsafe { libc::getpwuid_r(libc::geteuid(), &mut entry, buffer.as_mut_ptr().cast(), buffer.len(), &mut found) };
    if result != 0 || found.is_null() || entry.pw_dir.is_null() {
        return None;
    }
    let bytes = unsafe { std::ffi::CStr::from_ptr(entry.pw_dir) }.to_bytes();
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
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
        let command = value.get("command").and_then(Value::as_array).ok_or("unsafe-record")?;
        if command.len() != 1
            || command[0].as_str() != expected_releases.join("current/bin/sidevoice-connector").to_str()
        {
            return Err("unsafe-record");
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
fn open_reset_lock() -> Result<std::fs::File, &'static str> {
    let path = report_path().ok_or("report-unavailable")?.with_file_name("reset.lock");
    open_reset_lock_at(&path)
}

#[cfg(target_os = "macos")]
fn open_reset_lock_at(path: &Path) -> Result<std::fs::File, &'static str> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|_| "lock-unavailable")?;
    let meta = file.metadata().map_err(|_| "lock-unavailable")?;
    if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        return Err("unsafe-lock");
    }
    Ok(file)
}

#[cfg(target_os = "macos")]
fn try_lock(file: &std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

/// The helper inherits the locked file description. A new app waits for it before initializing writers.
#[cfg(target_os = "macos")]
pub fn await_reset_idle() -> bool {
    let Ok(file) = open_reset_lock() else { return false };
    let deadline = std::time::Instant::now() + Duration::from_secs(5 * 60);
    while std::time::Instant::now() < deadline {
        if try_lock(&file) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[cfg(target_os = "macos")]
fn inherit_lock(file: &std::fs::File) -> Result<i32, &'static str> {
    use std::os::fd::AsRawFd;
    let fd = file.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        return Err("lock-unavailable");
    }
    Ok(fd)
}

#[cfg(target_os = "macos")]
fn helper_lock(fd: Option<i32>) -> Option<std::fs::File> {
    let path = report_path()?.with_file_name("reset.lock");
    helper_lock_at(fd, &path)
}

#[cfg(target_os = "macos")]
fn helper_lock_at(fd: Option<i32>, path: &Path) -> Option<std::fs::File> {
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::MetadataExt;
    let fd = fd.filter(|fd| *fd >= 3)?;
    let held = unsafe { std::fs::File::from_raw_fd(fd) };
    let named = open_reset_lock_at(path).ok()?;
    let held_meta = held.metadata().ok()?;
    let named_meta = named.metadata().ok()?;
    if held_meta.dev() != named_meta.dev() || held_meta.ino() != named_meta.ino() {
        return None;
    }
    if !try_lock(&held) {
        return None;
    }
    use std::os::fd::AsRawFd;
    let flags = unsafe { libc::fcntl(held.as_raw_fd(), libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(held.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return None;
    }
    Some(held)
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
        home.join("Library/Application Support/Sidevoice/Models/blobs"),
        home.join("Library/Application Support/Sidevoice/Models/models"),
        home.join("Library/Application Support/Sidevoice/Models/partial"),
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
    // dup shares the directory offset with the held descriptor. A fresh open lets the
    // preflight and removal passes each enumerate the full directory.
    let copy = unsafe {
        libc::openat(fd, c".".as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
    };
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
            unsafe {
                *libc::__error() = 0;
            }
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                if unsafe { *libc::__error() } != 0 {
                    return Err("inspect");
                } else {
                    break;
                }
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
fn remove_model_store(root: &Path, home: &Path) -> Result<(), &'static str> {
    if root != home.join("Library/Application Support/Sidevoice/Models") {
        return Err("outside-known-dirs");
    }
    let children = ["blobs", "models", "partial"].map(|name| root.join(name));
    // Validate every owned child before removing any. Other programs may have their own siblings here.
    for child in &children {
        known_cleanup_path(child, home)?;
        guarded_tree(child, home, false, false, &mount_identity)?;
    }
    for child in &children {
        remove_known(child, home)?;
    }
    Ok(())
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
pub fn run_reset_helper(parent_pid: Option<u32>, lock_fd: Option<i32>) {
    #[cfg(target_os = "macos")]
    {
        let Some(lock) = helper_lock(lock_fd) else { return };
        let Some(parent_pid) = parent_pid else { return };
        if unsafe { libc::getppid() as u32 } != parent_pid {
            return;
        }
        let Some(report) = report_path() else { return };
        let webkit = webkit_children(parent_pid);
        if std::io::stdout().write_all(b"ready\n").is_err() {
            return;
        }
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
        if account_home().as_deref() != Some(home.as_path()) {
            return;
        }
        let mut completed = Vec::new();
        let mut failed = initial_failed.clone();
        let journal = report.with_file_name("journal.json");
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
            } else {
                let removed =
                    if name == "models" { remove_model_store(&path, &home) } else { remove_known(&path, &home) };
                match removed {
                    Ok(()) => completed.push(name),
                    Err(key) => failed.push(json!({"step": name, "key": key})),
                }
            }
            if write_atomic_json(&journal, &json!({"stage":"cleanup", "completed":completed, "failed":failed})).is_err()
            {
                failed.push(json!({"step":"report", "key":"reset.report-failed"}));
                break;
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
        finish_reset_at(
            &report,
            &journal,
            &completed,
            failed,
            initial.get("agents").cloned().unwrap_or_else(|| json!({})),
            write_atomic_json,
            || {
                // Cleanup is over. A new app must acquire this lock independently.
                drop(lock);
                std::env::current_exe()
                    .ok()
                    .and_then(|exe| Command::new(exe).env("SIDEVOICE_RESET_RELAUNCH", "1").spawn().ok())
                    .is_some()
            },
        );
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (parent_pid, lock_fd);
    }
}

#[cfg(target_os = "macos")]
fn finish_reset_at(
    report: &Path,
    journal: &Path,
    completed: &[&str],
    mut failed: Vec<Value>,
    agents: Value,
    write: impl Fn(&Path, &Value) -> Result<(), &'static str>,
    relaunch: impl FnOnce() -> bool,
) {
    // A successful spawn is not proof that the new app finished startup. Its setup code
    // publishes the final report; until then the journal is an interrupted partial result.
    let pending = json!({"stage":"awaiting-relaunch", "completed":completed, "failed":failed, "agents":agents});
    if write(journal, &pending).is_err() {
        return;
    }
    if !relaunch() {
        failed.push(json!({"step":"relaunch", "key":"reset.relaunch-failed"}));
        let value = json!({"state":"partial", "completed":completed, "failed":failed, "agents":agents});
        if write(report, &value).is_ok() {
            let _ = fs::remove_file(journal);
        }
    }
}

#[cfg(target_os = "macos")]
pub fn finalize_relaunched_reset() {
    if std::env::var("SIDEVOICE_RESET_RELAUNCH").as_deref() != Ok("1") {
        return;
    }
    let Some(report) = report_path() else { return };
    let journal = report.with_file_name("journal.json");
    let _ = finalize_relaunched_reset_at(&report, &journal);
}

#[cfg(target_os = "macos")]
fn finalize_relaunched_reset_at(report: &Path, journal: &Path) -> Result<(), &'static str> {
    let bytes = fs::read(journal).map_err(|_| "report-read")?;
    if bytes.len() > 16 * 1024 {
        return Err("report-read");
    }
    let pending: Value = serde_json::from_slice(&bytes).map_err(|_| "report-read")?;
    if pending["stage"] != "awaiting-relaunch" {
        return Err("report-stage");
    }
    let failed = pending.get("failed").and_then(Value::as_array).ok_or("report-stage")?;
    let value = json!({"state":if failed.is_empty() {"complete"} else {"partial"},
        "completed":pending["completed"],"failed":failed,"agents":pending["agents"]});
    write_atomic_json(report, &value)?;
    fs::remove_file(journal).map_err(|_| "report-remove")?;
    Ok(())
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
    fn shared_store_removes_only_engine_owned_children() {
        let home = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(home.path()).unwrap();
        let root = home.join("Library/Application Support/Sidevoice/Models");
        for child in ["blobs", "models", "partial"] {
            fs::create_dir_all(root.join(child)).unwrap();
            fs::write(root.join(child).join("owned"), b"model").unwrap();
        }
        fs::create_dir(root.join("other-program")).unwrap();
        fs::write(root.join("other-program/sentinel"), b"unrelated").unwrap();
        assert_eq!(remove_model_store(&root, &home), Ok(()));
        for child in ["blobs", "models", "partial"] {
            assert!(!root.join(child).exists());
        }
        assert_eq!(fs::read(root.join("other-program/sentinel")).unwrap(), b"unrelated");
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
        let releases = home.join(".local/share/sidevoice");
        fs::write(
            data.join("install.json"),
            json!({"releases": releases, "command": [releases.join("current/bin/sidevoice-connector")]}).to_string(),
        )
        .unwrap();
        assert!(validate_uninstall_scope_at(&home, &data, None).is_ok());
        fs::write(data.join("install.json"), json!({"releases": releases, "command": ["/usr/bin/rm"]}).to_string())
            .unwrap();
        assert_eq!(validate_uninstall_scope_at(&home, &data, None), Err("unsafe-record"));
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
    fn reset_and_model_or_credential_admission_are_atomic() {
        let mut gate = Admission::default();
        assert!(gate.enter()); // model install, voice start, or credential write
        assert!(!gate.begin_reset());
        gate.leave();
        assert!(gate.begin_reset());
        assert!(!gate.enter());
        gate.end_reset();
        assert!(gate.enter());
        gate.leave();
    }

    #[test]
    fn durable_journal_reports_interruption_then_allows_retry() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let report = dir.path().join("report.json");
        let journal = dir.path().join("journal.json");
        write_atomic_json(&journal, &json!({"stage":"service", "completed":["service"], "failed":[]})).unwrap();
        assert_eq!(fs::metadata(&journal).unwrap().permissions().mode() & 0o077, 0);
        let interrupted = consume_report_at(&report).unwrap();
        assert_eq!(interrupted["state"], "partial");
        assert_eq!(interrupted["completed"], json!(["service"]));
        assert_eq!(interrupted["failed"][0]["key"], "reset.interrupted");
        assert_eq!(consume_report_at(&report), Ok(Value::Null));
        write_atomic_json(&journal, &json!({"stage":"cleanup", "completed":["service", "models"], "failed":[]}))
            .unwrap();
        write_atomic_json(
            &report,
            &json!({"state":"complete", "completed":["service", "models"], "failed":[], "agents":{}}),
        )
        .unwrap();
        assert_eq!(consume_report_at(&report).unwrap()["state"], "complete");
        assert_eq!(consume_report_at(&report), Ok(Value::Null));
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), &report).unwrap();
        assert_eq!(consume_report_at(&report), Err(refused("reset.unsafe")));
        assert!(outside.path().exists());
    }

    #[test]
    fn helper_reports_complete_only_after_relaunched_setup_and_keeps_failed_writes() {
        let dir = tempfile::tempdir().unwrap();
        let report = dir.path().join("report.json");
        let journal = dir.path().join("journal.json");
        let mut launched = false;
        finish_reset_at(&report, &journal, &["service", "app-data"], vec![], json!({}), write_atomic_json, || {
            assert!(journal.exists());
            assert!(!report.exists());
            launched = true;
            true
        });
        assert!(launched);
        assert!(journal.exists());
        finalize_relaunched_reset_at(&report, &journal).unwrap();
        assert!(!journal.exists());
        assert_eq!(consume_report_at(&report).unwrap()["state"], "complete");

        let bad_write = |path: &Path, value: &Value| {
            if path == report {
                Err("report-write")
            } else {
                write_atomic_json(path, value)
            }
        };
        finish_reset_at(&report, &journal, &["service"], vec![], json!({}), bad_write, || true);
        assert!(journal.exists());
        assert_eq!(consume_report_at(&report).unwrap()["state"], "partial");

        finish_reset_at(&report, &journal, &["service"], vec![], json!({}), write_atomic_json, || false);
        let failed = consume_report_at(&report).unwrap();
        assert_eq!(failed["state"], "partial");
        assert_eq!(failed["failed"][0]["key"], "reset.relaunch-failed");
    }

    #[test]
    fn helper_lock_blocks_a_second_app_until_released() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reset.lock");
        let parent = open_reset_lock_at(&path).unwrap();
        let newcomer = open_reset_lock_at(&path).unwrap();
        assert!(try_lock(&parent));
        assert!(!try_lock(&newcomer));
        drop(parent);
        assert!(try_lock(&newcomer));
    }

    #[test]
    fn lock_helper_child_mode() {
        if std::env::var("SIDEVOICE_LOCK_HANDOFF_MODE").as_deref() != Ok("helper") {
            return;
        }
        let path = PathBuf::from(std::env::var_os("SIDEVOICE_LOCK_HANDOFF_PATH").unwrap());
        let fd: i32 = std::env::var("SIDEVOICE_LOCK_HANDOFF_FD").unwrap().parse().unwrap();
        let _held = helper_lock_at(Some(fd), &path).expect("helper validates and retains inherited lock");
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "data::tests::lock_relaunch_child_mode", "--nocapture"])
            .env("SIDEVOICE_LOCK_HANDOFF_MODE", "relaunch")
            .env("SIDEVOICE_LOCK_HANDOFF_PATH", &path)
            .env("SIDEVOICE_LOCK_HANDOFF_FD", fd.to_string())
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn lock_relaunch_child_mode() {
        if std::env::var("SIDEVOICE_LOCK_HANDOFF_MODE").as_deref() != Ok("relaunch") {
            return;
        }
        let path = PathBuf::from(std::env::var_os("SIDEVOICE_LOCK_HANDOFF_PATH").unwrap());
        let fd: i32 = std::env::var("SIDEVOICE_LOCK_HANDOFF_FD").unwrap().parse().unwrap();
        let mut inherited: libc::stat = unsafe { std::mem::zeroed() };
        let named = open_reset_lock_at(&path).unwrap();
        use std::os::fd::AsRawFd;
        let mut expected: libc::stat = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::fstat(named.as_raw_fd(), &mut expected) }, 0);
        let matches_lock = unsafe { libc::fstat(fd, &mut inherited) } == 0
            && inherited.st_dev == expected.st_dev
            && inherited.st_ino == expected.st_ino;
        assert!(!matches_lock, "relaunched process inherited the reset lock");
        assert!(!try_lock(&named), "helper keeps the lock until its work finishes");
    }

    #[test]
    fn reset_lock_handoff_closes_descriptor_before_relaunch() {
        let dir = tempfile::tempdir().unwrap();
        let path = fs::canonicalize(dir.path()).unwrap().join("reset.lock");
        let held = open_reset_lock_at(&path).unwrap();
        assert!(try_lock(&held));
        let fd = inherit_lock(&held).unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "data::tests::lock_helper_child_mode", "--nocapture"])
            .env("SIDEVOICE_LOCK_HANDOFF_MODE", "helper")
            .env("SIDEVOICE_LOCK_HANDOFF_PATH", &path)
            .env("SIDEVOICE_LOCK_HANDOFF_FD", fd.to_string())
            .spawn()
            .unwrap();
        drop(held);
        assert!(child.wait_with_output().unwrap().status.success());
        assert!(try_lock(&open_reset_lock_at(&path).unwrap()));
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
    fn production_process_parser_recognizes_bundled_core() {
        assert!(managed_process_in_ps(b"/Users/test/.local/share/sidevoice/current/core/bin/sidevoice-core-rust\n"));
        assert!(managed_process_in_ps(b"/Users/test/.local/share/sidevoice/current/bin/sidevoice-connector\n"));
        assert!(!managed_process_in_ps(b"/usr/bin/other-service\n"));
    }

    #[test]
    fn missing_record_with_residual_connector_data_refuses_absent_retry() {
        let home = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(home.path()).unwrap();
        let data = home.join(".sidevoice");
        assert!(!residual_connector_data(&home, &data));
        fs::create_dir(&data).unwrap();
        fs::write(data.join("install.lock"), b"marker").unwrap();
        assert!(!residual_connector_data(&home, &data));
        fs::write(data.join("core-secret"), b"sentinel").unwrap();
        assert!(residual_connector_data(&home, &data));
        fs::remove_file(data.join("core-secret")).unwrap();
        fs::create_dir_all(home.join(".local/share/sidevoice")).unwrap();
        assert!(residual_connector_data(&home, &data));
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
        let file = fs::canonicalize(home.path()).unwrap().join("app/pairings.json");
        let pending = json!({"fp":"fp", "public_key":"pk", "device_id":"id", "token":"live-secret", "urls":["https://example.test"]});
        let value = json!({"pairings":[], "pendingRevokes":[pending]});
        assert_eq!(write_pairings(&file, &value), Ok(Value::Null));
        assert_eq!(read_pairings(&file), Ok(value));
        assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o077, 0);
        assert_eq!(fs::metadata(file.parent().unwrap()).unwrap().permissions().mode() & 0o077, 0);
    }

    #[test]
    fn pairing_write_refuses_replaced_ancestor_before_rename() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(home.path()).unwrap();
        let outside = fs::canonicalize(outside.path()).unwrap();
        let app = home.join("app");
        fs::create_dir(&app).unwrap();
        fs::write(outside.join("sentinel"), b"unrelated").unwrap();
        let file = app.join("pairings.json");
        let value = json!({"pairings": [], "pendingRevokes": []});
        let result = write_pairings_unix(&file, &value, || {
            fs::rename(&app, home.join("moved-app")).unwrap();
            std::os::unix::fs::symlink(&outside, &app).unwrap();
        });
        assert_eq!(result, Err(refused("reset.unsafe")));
        assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"unrelated");
        assert!(!outside.join("pairings.json").exists());
    }
}
