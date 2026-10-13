//! `host.updater` (docs/BRIDGE.md → "The updater"): the schedule, the status the page reads and install-on-quit, over
//! tauri-plugin-updater (which checks `latest.json`, downloads, verifies and installs). The page is granted none of the
//! plugin's commands.

use serde_json::{json, Value};
use sidevoice_desktop_core::settings::{self, Settings};
use sidevoice_desktop_core::updater::{self as core, ErrorCode, Mode, State, Status};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, Webview};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::AppState;

const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const CHECK_FOR: Duration = Duration::from_secs(2 * 60);
const DOWNLOAD_FOR: Duration = Duration::from_secs(60 * 60);

/// The status and the update it is about (its bytes once ready), under one lock. `installing`: one install at a time,
/// and quitting waits for it.
#[derive(Default)]
struct Updates {
    status: Status,
    update: Option<Update>,
    bytes: Option<Vec<u8>>,
    installing: bool,
}

/// Only a release or a nightly (each built to read its own feed) that can replace itself: its `.app` run from its
/// bundle on a writable volume (not the .dmg, not translocated), its AppImage, its Windows install. Never a `.deb` or
/// a bare binary.
pub fn offered() -> bool {
    use tauri::utils::{config::BundleType, platform::bundle_type};
    static OFFERED: OnceLock<bool> = OnceLock::new();
    *OFFERED.get_or_init(|| {
        matches!(env!("SIDEVOICE_BUILD_CHANNEL"), "release" | "nightly")
            && match bundle_type() {
                Some(BundleType::App) => std::env::current_exe()
                    .is_ok_and(|exe| core::in_app_bundle(&exe) && exe.ancestors().nth(4).is_some_and(writable)),
                Some(BundleType::AppImage) => std::env::var_os("APPIMAGE").is_some(),
                Some(BundleType::Nsis) => true,
                _ => false,
            }
    })
}

fn writable(folder: &std::path::Path) -> bool {
    let probe = folder.join(format!(".sidevoice-update-check-{}", std::process::id()));
    let made = std::fs::OpenOptions::new().write(true).create_new(true).open(&probe);
    let _ = std::fs::remove_file(&probe);
    !made.is_err_and(|e| e.kind() == std::io::ErrorKind::ReadOnlyFilesystem)
}

/// At start: the plugin, then a check at launch and every 6 hours.
pub fn setup(app: &AppHandle) {
    if !offered() {
        return;
    }
    if let Err(e) = app.plugin(tauri_plugin_updater::Builder::new().build()) {
        return eprintln!("sidevoice: the updater did not start: {e}");
    }
    app.manage(Mutex::new(Updates::default()));
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            check(&app).await;
            tokio::time::sleep(EVERY).await;
        }
    });
}

fn mode(app: &AppHandle) -> Mode {
    app.state::<AppState>().settings.lock().unwrap().as_ref().map(|s| s.update_mode).unwrap_or_default()
}

/// Changes the updates and, still under the lock (so the page hears changes in order), tells the page a new status.
fn with<T>(app: &AppHandle, change: impl FnOnce(&mut Updates) -> T) -> T {
    let state = app.state::<Mutex<Updates>>();
    let mut updates = state.lock().unwrap();
    let before = updates.status.clone();
    let answer = change(&mut updates);
    if let Some(window) = (updates.status != before).then(|| crate::room_window(app)).flatten() {
        let status = json!(updates.status);
        let _ = window.eval(format!("window.__sidevoiceDesktop && window.__sidevoiceDesktop.updaterEvent({status})"));
    }
    answer
}

/// A newer version is `available` (automatic mode: downloaded at once); one downloading or ready is left as it is.
async fn check(app: &AppHandle) {
    let checked = async { app.updater_builder().timeout(CHECK_FOR).build()?.check().await }.await;
    with(app, |u| match checked {
        Ok(found) if matches!(u.status.state, State::Idle | State::Available) => {
            u.status = found.as_ref().map_or_else(Status::default, |f| Status::new(State::Available, &f.version));
            u.update = found;
        }
        Ok(_) => u.status.error = None,
        Err(e) => {
            crate::debug(&format!("updater check failed: {e}"));
            u.status = u.status.clone().failed(ErrorCode::CheckFailed);
        }
    });
    if mode(app) == Mode::Automatic {
        download(app);
    }
}

/// Downloads the update `available`, in the background.
fn download(app: &AppHandle) {
    let Some(mut update) = with(app, |u| match (u.status.state, &u.update) {
        (State::Available, Some(update)) => {
            u.status = Status::new(State::Downloading, &update.version);
            Some(update.clone())
        }
        _ => None,
    }) else {
        return;
    };
    update.timeout = Some(DOWNLOAD_FOR);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut received = 0;
        let progress = |chunk: usize, total: Option<u64>| {
            received += chunk as u64;
            with(&app, |u| u.status.progress = core::progress(received, total));
        };
        let downloaded = update.download(progress, || {}).await;
        with(&app, |u| match downloaded {
            Ok(bytes) => (u.status, u.bytes) = (Status::new(State::Ready, &update.version), Some(bytes)),
            Err(e) => {
                crate::debug(&format!("updater download failed: {e}"));
                u.status = Status::new(State::Available, &update.version).failed(ErrorCode::DownloadFailed);
            }
        });
    });
}

/// Installs the ready update (its bytes are the one install's: quitting waits for it); `restart`: then restarts into
/// it (Windows: the plugin exits and the installer starts the app). The local host stops only once it is installed.
fn install(app: &AppHandle, restart: bool) {
    let claimed = with(app, |u| u.update.clone().zip(u.bytes.take()).inspect(|_| u.installing = true));
    let Some((update, bytes)) = claimed else { return };
    let installed = update.restart_after_install(restart).install(&bytes);
    with(app, |u| {
        u.installing = false;
        if let Err(e) = &installed {
            crate::debug(&format!("updater install failed: {e}"));
            (u.bytes, u.status) = (Some(bytes), u.status.clone().failed(ErrorCode::InstallFailed));
        }
    });
    if installed.is_ok() && restart {
        crate::local_host::shutdown(app);
        app.restart()
    }
}

/// Quitting (`RunEvent::Exit`): waits for an install under way (a restart's); in automatic mode installs one ready.
pub fn quit(app: &AppHandle) {
    if app.try_state::<Mutex<Updates>>().is_none() {
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(5 * 60);
    while with(app, |u| u.installing) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    if mode(app) == Mode::Automatic {
        install(app, false);
    }
}

fn unavailable() -> Value {
    json!({ "key": "update_unavailable", "message": "There is no update to install." })
}

/// Only the current room window on the app's own page, and only where the updater runs.
fn caller(app: &AppHandle, webview: &Webview) -> Result<Status, Value> {
    crate::room_page(app, webview).map_err(|why| json!({ "key": "bad_request", "message": why }))?;
    app.try_state::<Mutex<Updates>>().ok_or_else(unavailable)?;
    Ok(with(app, |u| u.status.clone()))
}

#[tauri::command]
pub fn updater_settings(app: AppHandle, webview: Webview) -> Result<Value, Value> {
    caller(&app, &webview)?;
    Ok(json!({ "mode": mode(&app) }))
}

/// Keeps the mode with the app's settings; automatic starts downloading an update already found.
#[tauri::command]
pub fn updater_set_mode(app: AppHandle, webview: Webview, mode: Mode) -> Result<Value, Value> {
    caller(&app, &webview)?;
    let internal = |message: String| json!({ "key": "internal", "message": message });
    let dir = app.path().app_config_dir().map_err(|e| internal(e.to_string()))?;
    let state = app.state::<AppState>();
    let mut stored = state.settings.lock().unwrap();
    let changed = Settings { update_mode: mode, ..stored.clone().unwrap_or_else(Settings::first_run) };
    settings::save(&dir, &changed).map_err(|e| internal(format!("not saved: {e}")))?;
    *stored = Some(changed);
    drop(stored);
    if mode == Mode::Automatic {
        download(&app);
    }
    Ok(json!({ "mode": mode }))
}

#[tauri::command]
pub fn updater_status(app: AppHandle, webview: Webview) -> Result<Status, Value> {
    caller(&app, &webview)
}

/// "Check for updates": the status once the check is done.
#[tauri::command]
pub async fn updater_check(app: AppHandle, webview: Webview) -> Result<Status, Value> {
    caller(&app, &webview)?;
    check(&app).await;
    caller(&app, &webview)
}

/// Starts downloading the update found; `update_unavailable` when none is.
#[tauri::command]
pub fn updater_download(app: AppHandle, webview: Webview) -> Result<Status, Value> {
    caller(&app, &webview)?;
    download(&app);
    caller(&app, &webview).and_then(|s| if s.state == State::Idle { Err(unavailable()) } else { Ok(s) })
}

/// "Restart to update": installs the ready update and restarts now; `update_unavailable` unless one is ready.
#[tauri::command]
pub fn updater_restart(app: AppHandle, webview: Webview) -> Result<(), Value> {
    if caller(&app, &webview)?.state != State::Ready {
        return Err(unavailable());
    }
    tauri::async_runtime::spawn_blocking(move || install(&app, true));
    Ok(())
}
