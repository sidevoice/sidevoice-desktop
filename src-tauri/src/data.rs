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

/// The models folder the engine and the app share: `Models` by the one rule of `sidevoice-platform` (always the OS standard
/// place, the same on every OS, independent of `$SIDEVOICE_DATA_DIR`). `None` when this user has none.
pub fn shared_model_root() -> Option<PathBuf> {
    sidevoice_platform::data_dir::models_dir()
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
        write_pairings_unix(path, value, || {})
    }
    #[cfg(windows)]
    {
        write_pairings_windows(path, value, || {})
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
        confirmed_reset(app, false).await
    }
}

/// `deleteAll()` and then the app bundle: the helper moves it to the Trash after the app quits and does not relaunch.
#[tauri::command]
pub async fn data_uninstall(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err(refused("reset.unavailable"))
    }
    #[cfg(target_os = "macos")]
    {
        confirmed_reset(app, true).await
    }
}

#[cfg(target_os = "macos")]
async fn confirmed_reset(app: AppHandle, uninstall: bool) -> Answer {
    use crate::platform::confirmation::{Confirmation, ConfirmationKind, NativeConfirmation};
    use crate::platform::webview_data::{SystemWebviewData, WebviewData};
    let state = app.state::<AppState>();
    if !begin_exclusive(&app) {
        return Err(refused("reset.busy"));
    }
    if let Err(error) = SystemWebviewData::preflight(std::process::id()) {
        end_exclusive(&app);
        // Existing translated guidance: open the installed app from Applications, then retry.
        let key = if error.kind() == std::io::ErrorKind::Unsupported {
            "reset.app-unremovable"
        } else {
            "reset.helper-failed"
        };
        return Err(refused(key));
    }
    if state.call.lock().unwrap().joined
        || crate::local_host::installing(&app)
        || crate::updater::installing(&app)
        || app.try_state::<crate::engine_ipc::EngineState>().is_some_and(|engine| engine.jobs.busy())
    {
        end_exclusive(&app);
        return Err(refused("reset.busy"));
    }
    // An app that cannot remove its own bundle (a disk image, a translocated or read-only copy) refuses
    // before the confirmation and before any data is touched.
    if uninstall {
        if let Err(key) = removable_app_bundle() {
            end_exclusive(&app);
            return Err(refused(key));
        }
    }
    if let Err(refusal) = require_confirmation(
        NativeConfirmation::confirm(
            &app,
            if uninstall { ConfirmationKind::Uninstall } else { ConfirmationKind::DeleteAllData },
        )
        .await,
    ) {
        end_exclusive(&app);
        return Err(refusal);
    }
    let app_for_work = app.clone();
    let answer = tauri::async_runtime::spawn_blocking(move || begin_reset(&app_for_work, uninstall))
        .await
        .unwrap_or_else(|_| Err(refused("reset.failed")));
    if answer.is_err() {
        end_exclusive(&app);
    }
    answer
}

#[cfg(target_os = "macos")]
fn require_confirmation(accepted: bool) -> Result<(), Value> {
    accepted.then_some(()).ok_or_else(|| refused("reset.cancelled"))
}

/// The `.app` bundle this executable runs from: `<bundle>.app/Contents/MacOS/<executable>`. Anything else (a
/// development build, a path with `..`, a translocated copy) has no bundle the uninstall may remove.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn app_bundle_of(exe: &Path) -> Result<PathBuf, &'static str> {
    let clean = exe.is_absolute()
        && exe.components().all(|part| matches!(part, std::path::Component::RootDir | std::path::Component::Normal(_)));
    if !clean || exe.components().any(|part| part.as_os_str() == "AppTranslocation") {
        return Err("reset.app-unremovable");
    }
    let macos = exe.parent().filter(|dir| dir.file_name().is_some_and(|name| name == "MacOS"));
    let contents = macos.and_then(Path::parent).filter(|dir| dir.file_name().is_some_and(|name| name == "Contents"));
    let bundle = contents.and_then(Path::parent).filter(|dir| {
        dir.extension().is_some_and(|ext| ext == "app") && dir.parent().is_some_and(|parent| parent != Path::new("/"))
    });
    bundle.map(Path::to_path_buf).ok_or("reset.app-unremovable")
}

/// This app's own bundle, checked again by the helper right before it moves it: a real directory under its
/// canonical path, with its Info.plist, in a folder this user may write.
#[cfg(target_os = "macos")]
fn removable_app_bundle() -> Result<PathBuf, &'static str> {
    use std::os::unix::ffi::OsStrExt;
    let exe = std::env::current_exe().and_then(fs::canonicalize).map_err(|_| "reset.app-unremovable")?;
    let bundle = app_bundle_of(&exe)?;
    let meta = fs::symlink_metadata(&bundle).map_err(|_| "reset.app-unremovable")?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || fs::canonicalize(&bundle).ok().as_deref() != Some(bundle.as_path())
        || !bundle.join("Contents/Info.plist").is_file()
    {
        return Err("reset.app-unremovable");
    }
    let parent = bundle.parent().ok_or("reset.app-unremovable")?;
    let parent = std::ffi::CString::new(parent.as_os_str().as_bytes()).map_err(|_| "reset.app-unremovable")?;
    if unsafe { libc::access(parent.as_ptr(), libc::W_OK) } != 0 {
        return Err("reset.app-unremovable");
    }
    Ok(bundle)
}

/// Finder's own move to the Trash (`NSFileManager trashItemAtURL:`), so the app can be put back from there.
#[cfg(target_os = "macos")]
fn move_to_trash(bundle: &Path) -> Result<(), &'static str> {
    use objc2::{class, msg_send, rc::autoreleasepool, runtime::AnyObject};
    use objc2_foundation::NSString;
    let text = bundle.to_str().ok_or("reset.app-unremovable")?;
    let moved = autoreleasepool(|_| unsafe {
        let manager: *mut AnyObject = msg_send![class!(NSFileManager), defaultManager];
        let path = NSString::from_str(text);
        let url: *mut AnyObject = msg_send![class!(NSURL), fileURLWithPath: &*path, isDirectory: true];
        if manager.is_null() || url.is_null() {
            return false;
        }
        let resulting: *mut *mut AnyObject = std::ptr::null_mut();
        let error: *mut *mut AnyObject = std::ptr::null_mut();
        let moved: bool = msg_send![manager, trashItemAtURL: url, resultingItemURL: resulting, error: error];
        moved
    });
    if moved {
        Ok(())
    } else {
        Err("reset.app-trash-failed")
    }
}

#[cfg(target_os = "macos")]
fn begin_reset(app: &AppHandle, uninstall: bool) -> Answer {
    use sidevoice_local_host::{cli::Cli, paths::DataDirs};
    let reset_lock = open_reset_lock().map_err(|_| refused("reset.helper-failed"))?;
    if !try_lock(&reset_lock) {
        return Err(refused("reset.busy"));
    }
    let dirs = DataDirs::from_env().ok_or_else(|| refused("reset.unavailable"))?;
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| refused("reset.unavailable"))?;
    validate_uninstall_scope(&home, &dirs.data).map_err(|reason| json!({"key": "reset.unsafe", "reason": reason}))?;
    let installed_cli = match Cli::installed(&dirs) {
        Ok(cli) => {
            probe_safe_connector(&cli, Duration::from_secs(10))?;
            Some(cli)
        }
        Err(_) if !dirs.install_record().exists() => {
            // A missing record is only an absent service when all owned data is gone too.
            if !fresh_service_absent(&home) || residual_connector_data(&dirs.data) {
                return Err(refused("reset.service-failed"));
            }
            None
        }
        Err(_) => return Err(refused("reset.unsafe")),
    };
    let report = report_path().ok_or_else(|| refused("reset.helper-failed"))?;
    if let Err(error) = fs::remove_file(&report) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(refused("reset.report-failed"));
        }
    }
    let journal = report.with_file_name("journal.json");
    write_atomic_json(&journal, &json!({"stage":"preflight", "completed":[], "failed":[]}))
        .map_err(|_| refused("reset.helper-failed"))?;
    let (service, service_outcome) = match installed_cli {
        Some(cli) => (
            run_checked_uninstall(&home, &dirs.data, || {
                cli.run(&["uninstall", "--json"], Duration::from_secs(180)).map_err(|_| refused("reset.service-failed"))
            })?,
            "removed",
        ),
        None => (json!({"state": "absent", "agents": {}}), "absent"),
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
        .args(uninstall.then_some("uninstall"))
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
    let mut pending = vec!["app-data", "models", "webview", "cache", "saved-state", "logs", "preferences"];
    if uninstall {
        pending.push("app");
    }
    let state = match (failed.is_empty(), uninstall) {
        (false, _) => "partial",
        (true, false) => "needs-restart",
        (true, true) => "needs-quit",
    };
    Ok(json!({"state": state, "service": service_outcome, "agents": agents, "failed": failed, "pending": pending}))
}

#[cfg(target_os = "macos")]
fn require_safe_uninstall(version: &Value) -> Result<(), Value> {
    if version.get("ok") == Some(&Value::Bool(true))
        && version.get("safe_uninstall").and_then(Value::as_u64).is_some_and(|level| level >= 1)
    {
        Ok(())
    } else {
        Err(refused("reset.unsupported-connector"))
    }
}

#[cfg(target_os = "macos")]
fn probe_safe_connector(cli: &sidevoice_local_host::cli::Cli, timeout: Duration) -> Result<(), Value> {
    let version = cli.run(&["--version", "--json"], timeout).map_err(|_| refused("reset.unsupported-connector"))?;
    require_safe_uninstall(&version)
}

#[cfg(target_os = "macos")]
fn residual_connector_data(data: &Path) -> bool {
    // A missing record is not proof that an interrupted uninstall finished its data phase: the releases (`D/install`) are
    // an entry of `D` too.
    let entries = match fs::read_dir(data) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    let models = sidevoice_platform::data_dir::MODELS;
    for entry in entries {
        let Ok(entry) = entry else { return true };
        let name = entry.file_name();
        // `Models` is the engine's (docs/ENGINES.md); on macOS its place is `D`: not the connector's to leave behind.
        if name == models {
            continue;
        }
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
    validate_uninstall_scope_at(home, data)
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

/// `D` on macOS with no override: the OS's standard place of `sidevoice-platform`'s `data_dir`.
#[cfg(target_os = "macos")]
fn standard_data(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Sidevoice")
}

/// The shared models folder as [`shared_model_root`] places it for this home (the OS standard place; on macOS it is in `D`).
#[cfg(target_os = "macos")]
fn standard_models(home: &Path) -> PathBuf {
    standard_data(home).join(sidevoice_platform::data_dir::MODELS)
}

#[cfg(target_os = "macos")]
fn validate_uninstall_scope_at(home: &Path, data: &Path) -> Result<PathBuf, &'static str> {
    if !clean_absolute(home) || fs::canonicalize(home).ok().as_deref() != Some(home) {
        return Err("unsafe-home");
    }
    // `D` on macOS: the standard place of `sidevoice-platform`'s `data_dir`, and the releases `D/install` in it. An
    // override (`SIDEVOICE_DATA_DIR`) is not a known directory.
    let expected_data = standard_data(home);
    if data != expected_data || !clean_absolute(data) {
        return Err("outside-known-dirs");
    }
    let expected_releases = sidevoice_local_host::paths::DataDirs::new(&expected_data).releases();
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
        let recovery = expected_data.join(".uninstall-recovery/sidevoice-connector");
        if command.len() != 1
            || !matches!(command[0].as_str(), Some(path) if is_release_binary(Path::new(path), &expected_releases) || Path::new(path) == recovery)
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

/// `R/releases/<id>/bin/sidevoice-connector`: the command the connector's `install` records for the release it
/// selects, `<id>` one plain name.
#[cfg(target_os = "macos")]
fn is_release_binary(path: &Path, releases: &Path) -> bool {
    use std::path::Component;
    let Ok(rest) = path.strip_prefix(releases.join("releases")) else { return false };
    let parts: Vec<_> = rest.components().collect();
    matches!(parts.as_slice(), [Component::Normal(id), Component::Normal(bin), Component::Normal(name)]
        if !id.to_string_lossy().starts_with('.') && *bin == "bin" && *name == "sidevoice-connector")
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
        standard_models(home).join("blobs"),
        standard_models(home).join("models"),
        standard_models(home).join("partial"),
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
    // Recheck the exact top-level name immediately before guarded traversal. A missing
    // directory is already gone; a link, alias or changed ancestor is a reported refusal.
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if !meta.is_dir() || meta.file_type().is_symlink() || fs::canonicalize(path).ok().as_deref() != Some(path) {
                return Err("changed-path");
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("inspect"),
    }
    guarded_tree(path, home, true, false, probe)
}

#[cfg(target_os = "macos")]
fn remove_known(path: &Path, home: &Path) -> Result<(), &'static str> {
    remove_known_with(path, home, &mount_identity)
}

#[cfg(target_os = "macos")]
fn remove_model_store(root: &Path, home: &Path) -> Result<(), &'static str> {
    if root != standard_models(home) {
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

/// Called in the helper process, after its parent closes the pipe on exit. `uninstall`: then the app bundle too.
pub fn run_reset_helper(parent_pid: Option<u32>, lock_fd: Option<i32>, uninstall: bool) {
    #[cfg(target_os = "macos")]
    {
        let Some(lock) = helper_lock(lock_fd) else { return };
        let Some(parent_pid) = parent_pid else { return };
        if unsafe { libc::getppid() as u32 } != parent_pid {
            return;
        }
        let Some(report) = report_path() else { return };
        use crate::platform::webview_data::{SystemWebviewData, WebviewData};
        let Some(home) = account_home() else { return };
        let browser =
            SystemWebviewData::locations(&home, &home.join("Library/Application Support/dev.sidevoice.desktop"))
                .and_then(|paths| SystemWebviewData::capture(parent_pid, paths));
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
        let browser = browser.and_then(|owner| {
            SystemWebviewData::wait_released(owner, std::time::Instant::now() + Duration::from_secs(10))
        });
        let paths = [
            ("app-data", home.join("Library/Application Support/dev.sidevoice.desktop")),
            ("models", standard_models(&home)),
            ("webview", home.join("Library/WebKit/dev.sidevoice.desktop")),
            ("cache", home.join("Library/Caches/dev.sidevoice.desktop")),
            ("saved-state", home.join("Library/Saved Application State/dev.sidevoice.desktop.savedState")),
            ("logs", home.join("Library/Logs/dev.sidevoice.desktop")),
        ];
        for (name, path) in paths {
            let browser_storage = matches!(name, "app-data" | "webview" | "cache");
            if browser_storage && browser.is_err() {
                failed.push(json!({"step": name, "key": "reset.process-busy"}));
            } else {
                let removed = if browser_storage {
                    SystemWebviewData::remove(browser.as_ref().unwrap(), &path).map_err(|_| "remove")
                } else if name == "models" {
                    remove_model_store(&path, &home)
                } else {
                    remove_known(&path, &home)
                };
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
        let agents = initial.get("agents").cloned().unwrap_or_else(|| json!({}));
        let relaunch = || {
            // Cleanup is over. A new app must acquire this lock independently.
            drop(lock);
            std::env::current_exe()
                .ok()
                .and_then(|exe| Command::new(exe).env("SIDEVOICE_RESET_RELAUNCH", "1").spawn().ok())
                .is_some()
        };
        if uninstall {
            let trash = || removable_app_bundle().and_then(|bundle| move_to_trash(&bundle));
            finish_uninstall_at(&report, &journal, &completed, failed, agents, write_atomic_json, trash, relaunch);
        } else {
            finish_reset_at(&report, &journal, &completed, failed, agents, write_atomic_json, relaunch);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (parent_pid, lock_fd, uninstall);
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
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

/// The uninstall's last step. Only a clean run moves the app to the Trash, and it leaves no report: nothing is left
/// to read it. A failed data step (anything but a retained agent registration, which the confirmation already names)
/// keeps the app, and so does a failed move: the app relaunches, as after `deleteAll()`, and reports it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[allow(clippy::too_many_arguments)]
fn finish_uninstall_at(
    report: &Path,
    journal: &Path,
    completed: &[&str],
    mut failed: Vec<Value>,
    agents: Value,
    write: impl Fn(&Path, &Value) -> Result<(), &'static str>,
    trash: impl FnOnce() -> Result<(), &'static str>,
    relaunch: impl FnOnce() -> bool,
) {
    if failed.iter().any(|row| row["step"] != "agents") {
        failed.push(json!({"step":"app", "key":"reset.app-kept"}));
    } else if write(journal, &json!({"stage":"app", "completed":completed, "failed":failed, "agents":agents})).is_err()
    {
        failed.push(json!({"step":"report", "key":"reset.report-failed"}));
        failed.push(json!({"step":"app", "key":"reset.app-kept"}));
    } else {
        match trash() {
            Ok(()) => {
                let _ = fs::remove_file(journal);
                let _ = fs::remove_file(report);
                return;
            }
            Err(key) => failed.push(json!({"step":"app", "key":key})),
        }
    }
    finish_reset_at(report, journal, completed, failed, agents, write, relaunch);
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
        let root = standard_models(&home);
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
        let data = home.join("Library/Application Support/Sidevoice");
        assert_eq!(
            validate_uninstall_scope_at(&home, &home.join(".sidevoice")),
            Err("outside-known-dirs"),
            "the old directory is not known"
        );
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("install.json"), json!({"releases": home.join("Documents/sidevoice")}).to_string())
            .unwrap();
        assert_eq!(validate_uninstall_scope_at(&home, &data), Err("outside-known-dirs"));
        let releases = data.join("install");
        fs::write(
            data.join("install.json"),
            json!({"releases": releases, "command": [releases.join("releases/1.2.3/bin/sidevoice-connector")]})
                .to_string(),
        )
        .unwrap();
        assert!(validate_uninstall_scope_at(&home, &data).is_ok());
        fs::write(
            data.join("install.json"),
            json!({"releases": releases, "command": [data.join(".uninstall-recovery/sidevoice-connector")]})
                .to_string(),
        )
        .unwrap();
        assert!(validate_uninstall_scope_at(&home, &data).is_ok());
        fs::write(data.join("install.json"), json!({"releases": releases, "command": ["/usr/bin/rm"]}).to_string())
            .unwrap();
        assert_eq!(validate_uninstall_scope_at(&home, &data), Err("unsafe-record"));
        // Not the layout the connector records: `current` is a link of the old installer, not a release.
        fs::write(
            data.join("install.json"),
            json!({"releases": releases, "command": [releases.join("current/bin/sidevoice-connector")]}).to_string(),
        )
        .unwrap();
        assert_eq!(validate_uninstall_scope_at(&home, &data), Err("unsafe-record"));
    }

    #[test]
    fn installed_connector_must_advertise_safe_uninstall_before_reset() {
        for value in [
            json!({"ok":true,"version":"0.6.0"}),
            json!({"ok":true,"safe_uninstall":0}),
            json!({"ok":true,"safe_uninstall":"1"}),
            json!({"ok":true,"safe_uninstall":1.0}),
            json!({"ok":false,"safe_uninstall":1}),
        ] {
            assert_eq!(require_safe_uninstall(&value), Err(refused("reset.unsupported-connector")));
        }
        assert_eq!(require_safe_uninstall(&json!({"ok":true,"safe_uninstall":1})), Ok(()));
        assert_eq!(require_safe_uninstall(&json!({"ok":true,"safe_uninstall":2})), Ok(()));
    }

    #[test]
    fn fake_installed_connectors_refuse_without_safe_capability() {
        use sidevoice_local_host::cli::Cli;
        use std::os::unix::fs::PermissionsExt;
        let cases = [
            ("missing", r#"{"ok":true,"version":"0.6.0"}"#, false),
            ("zero", r#"{"ok":true,"safe_uninstall":0}"#, false),
            ("one", r#"{"ok":true,"safe_uninstall":1}"#, true),
            ("two", r#"{"ok":true,"safe_uninstall":2}"#, true),
            ("garbage", "not-json", false),
        ];
        for (name, answer, accepted) in cases {
            let dir = tempfile::tempdir().unwrap();
            let script = dir.path().join("sidevoice-connector");
            let marker = dir.path().join("uninstall-invoked");
            fs::write(
                &script,
                format!(
                    "#!/bin/sh\nif [ \"$1\" = \"--version\" ] && [ \"$2\" = \"--json\" ]; then\n  printf '%s\\n' '{answer}'\nelse\n  : > '{}'\n  printf '%s\\n' '{{\"ok\":true}}'\nfi\n",
                    marker.display()
                ),
            )
            .unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
            let cli = Cli { prefix: vec![script.to_string_lossy().into_owned()], data: dir.path().join(".sidevoice") };
            let checked = probe_safe_connector(&cli, Duration::from_secs(2));
            assert_eq!(checked.is_ok(), accepted, "{name}");
            if checked.is_ok() {
                cli.run(&["uninstall", "--json"], Duration::from_secs(2)).unwrap();
            }
            assert_eq!(marker.exists(), accepted, "{name}: unsafe uninstall invocation");
        }
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("slow-sidevoice-connector");
        fs::write(&script, "#!/bin/sh\nsleep 2\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let cli = Cli { prefix: vec![script.to_string_lossy().into_owned()], data: dir.path().join(".sidevoice") };
        assert_eq!(probe_safe_connector(&cli, Duration::from_millis(100)), Err(refused("reset.unsupported-connector")));
    }

    #[test]
    fn retained_connector_record_uses_recovery_executable_on_retry() {
        use sidevoice_local_host::{cli::Cli, paths::DataDirs};
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(home.path()).unwrap();
        let data = home.join("Library/Application Support/Sidevoice");
        let recovery = data.join(".uninstall-recovery");
        fs::create_dir_all(&recovery).unwrap();
        let script = recovery.join("sidevoice-connector");
        let marker = home.join("retry-invoked");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf '%s\\n' '{{\"ok\":true,\"safe_uninstall\":1}}'\nelse\n  : > '{}'\n  printf '%s\\n' '{{\"ok\":true,\"state\":\"removed\"}}'\nfi\n",
                marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let releases = data.join("install");
        fs::write(data.join("install.json"), json!({"releases": releases.clone(), "command": [script]}).to_string())
            .unwrap();
        assert_eq!(validate_uninstall_scope_at(&home, &data), Ok(releases));
        let cli = Cli::installed(&DataDirs::new(&data)).unwrap();
        assert_eq!(probe_safe_connector(&cli, Duration::from_secs(2)), Ok(()));
        assert!(!marker.exists());
        assert_eq!(cli.run(&["uninstall", "--json"], Duration::from_secs(2)).unwrap()["state"], "removed");
        assert!(marker.exists());
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
        // Inspect the inherited number before opening the named lock: that open may reuse a
        // correctly closed descriptor number and otherwise make the test report a false leak.
        let inherited_open = unsafe { libc::fstat(fd, &mut inherited) } == 0;
        let named = open_reset_lock_at(&path).unwrap();
        use std::os::fd::AsRawFd;
        let mut expected: libc::stat = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::fstat(named.as_raw_fd(), &mut expected) }, 0);
        let matches_lock = inherited_open && inherited.st_dev == expected.st_dev && inherited.st_ino == expected.st_ino;
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
        assert!(managed_process_in_ps(
            b"/Users/test/Library/Application Support/Sidevoice/install/releases/1.2.3/core/bin/sidevoice-core-rust\n"
        ));
        assert!(managed_process_in_ps(
            b"/Users/test/Library/Application Support/Sidevoice/install/releases/1.2.3/bin/sidevoice-connector\n"
        ));
        assert!(!managed_process_in_ps(b"/usr/bin/other-service\n"));
    }

    #[test]
    fn missing_record_with_residual_connector_data_refuses_absent_retry() {
        let home = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(home.path()).unwrap();
        let data = home.join("Library/Application Support/Sidevoice");
        assert!(!residual_connector_data(&data));
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("install.lock"), b"marker").unwrap();
        assert!(!residual_connector_data(&data));
        fs::write(data.join("core-secret"), b"sentinel").unwrap();
        assert!(residual_connector_data(&data));
        fs::remove_file(data.join("core-secret")).unwrap();
        fs::create_dir(data.join(".uninstall-recovery")).unwrap();
        assert!(residual_connector_data(&data));
        fs::remove_dir(data.join(".uninstall-recovery")).unwrap();
        fs::create_dir_all(data.join("install")).unwrap();
        assert!(residual_connector_data(&data));
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

/// The uninstall's platform-neutral decisions run on every platform's tests; the macOS-only parts above do not.
#[cfg(test)]
mod uninstall_tests {
    use super::*;

    fn plain_write(path: &Path, value: &Value) -> Result<(), &'static str> {
        fs::write(path, serde_json::to_vec(value).unwrap()).map_err(|_| "report-write")
    }

    fn read(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn only_a_real_app_bundle_can_be_removed() {
        let ok = |exe: &str| app_bundle_of(Path::new(exe));
        assert_eq!(
            ok("/Applications/Sidevoice.app/Contents/MacOS/sidevoice-desktop"),
            Ok(PathBuf::from("/Applications/Sidevoice.app"))
        );
        assert_eq!(
            ok("/Users/ana/Applications/Sidevoice Nightly.app/Contents/MacOS/sidevoice-desktop"),
            Ok(PathBuf::from("/Users/ana/Applications/Sidevoice Nightly.app"))
        );
        for refused in [
            // A development build has no bundle.
            "/Users/ana/sidevoice-desktop/src-tauri/target/debug/sidevoice-desktop",
            "/Applications/Sidevoice/Contents/MacOS/sidevoice-desktop",
            "/Applications/Sidevoice.app/Contents/Resources/sidevoice-desktop",
            "/Applications/Sidevoice.app/MacOS/sidevoice-desktop",
            "/Sidevoice.app/Contents/MacOS/sidevoice-desktop",
            "/Applications/../Sidevoice.app/Contents/MacOS/sidevoice-desktop",
            "Sidevoice.app/Contents/MacOS/sidevoice-desktop",
            "/private/var/folders/x/T/AppTranslocation/ABC/d/Sidevoice.app/Contents/MacOS/sidevoice-desktop",
        ] {
            assert_eq!(ok(refused), Err("reset.app-unremovable"), "{refused}");
        }
    }

    #[test]
    fn a_clean_uninstall_trashes_the_app_without_relaunch_or_report() {
        let dir = tempfile::tempdir().unwrap();
        let (report, journal) = (dir.path().join("report.json"), dir.path().join("journal.json"));
        let mut trashed = false;
        let agents = json!({"codex": "foreign"});
        let failed = vec![json!({"step": "agents", "key": "reset.agent-remains", "agent": "codex"})];
        finish_uninstall_at(
            &report,
            &journal,
            &["service", "app-data"],
            failed,
            agents,
            |path, value| {
                assert_eq!(value["stage"], "app");
                plain_write(path, value)
            },
            || {
                assert!(journal.exists(), "the journal names the step in progress");
                trashed = true;
                Ok(())
            },
            || panic!("an uninstalled app is not relaunched"),
        );
        assert!(trashed);
        assert!(!journal.exists() && !report.exists());
    }

    #[test]
    fn a_failed_data_step_keeps_the_app_and_relaunches_to_report() {
        let dir = tempfile::tempdir().unwrap();
        let (report, journal) = (dir.path().join("report.json"), dir.path().join("journal.json"));
        let mut relaunched = false;
        finish_uninstall_at(
            &report,
            &journal,
            &["service"],
            vec![json!({"step": "webview", "key": "reset.process-busy"})],
            json!({}),
            plain_write,
            || panic!("an app whose data remains is not trashed"),
            || {
                relaunched = true;
                true
            },
        );
        assert!(relaunched);
        let pending = read(&journal);
        assert_eq!(pending["stage"], "awaiting-relaunch");
        assert_eq!(pending["failed"][1], json!({"step": "app", "key": "reset.app-kept"}));
    }

    #[test]
    fn a_failed_move_relaunches_and_reports_it() {
        let dir = tempfile::tempdir().unwrap();
        let (report, journal) = (dir.path().join("report.json"), dir.path().join("journal.json"));
        finish_uninstall_at(
            &report,
            &journal,
            &["service"],
            vec![],
            json!({}),
            plain_write,
            || Err("reset.app-trash-failed"),
            || true,
        );
        let pending = read(&journal);
        assert_eq!(pending["stage"], "awaiting-relaunch");
        assert_eq!(pending["failed"], json!([{"step": "app", "key": "reset.app-trash-failed"}]));

        // Neither moved nor relaunched: the next launch reads a partial report.
        fs::remove_file(&journal).unwrap();
        finish_uninstall_at(
            &report,
            &journal,
            &["service"],
            vec![],
            json!({}),
            plain_write,
            || Err("reset.app-trash-failed"),
            || false,
        );
        let value = read(&report);
        assert_eq!(value["state"], "partial");
        assert_eq!(value["failed"][0]["key"], "reset.app-trash-failed");
        assert_eq!(value["failed"][1]["key"], "reset.relaunch-failed");
    }
}
