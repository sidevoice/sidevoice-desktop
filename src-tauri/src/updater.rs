//! The app's updates (docs/BRIDGE.md → "The updater"): `window.__sidevoiceDesktop.host.updater`, a thin layer over
//! tauri-plugin-updater (from Rust only: the page is granted none of its own commands). The plugin reads the release's
//! `latest.json` (tauri.conf.json `plugins.updater`), downloads and verifies the update, and installs it; this keeps
//! the schedule (at launch, then every 6 hours), the status the page reads, and install-on-quit. The mode is the app's
//! settings' (`update_mode`).
//!
//! Only a numbered release has it (`SIDEVOICE_BUILD_CHANNEL`, build.rs; RELEASING.md → "Channels"), and only where
//! that copy can replace itself ([`offered`]). Elsewhere nothing is checked and the page has no `host.updater`.

use serde_json::{json, Value};
use sidevoice_desktop_core::build_info::{BuildInfo, Channel};
use sidevoice_desktop_core::settings::{self, Settings};
use sidevoice_desktop_core::updater::{self as core, ErrorCode, Mode, State, Status};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, Webview};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::AppState;

/// Checked at launch and then this often.
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// A check or a download that hangs is given up (the next check tries again).
const CHECK_FOR: Duration = Duration::from_secs(2 * 60);
const DOWNLOAD_FOR: Duration = Duration::from_secs(60 * 60);

/// `host.build`: the channel, the app's version (a nightly's is the last release's), and a nightly's commit and date.
pub fn build(app: &AppHandle) -> BuildInfo {
    BuildInfo::new(
        env!("SIDEVOICE_BUILD_CHANNEL"),
        &app.package_info().version.to_string(),
        env!("SIDEVOICE_BUILD_COMMIT"),
        env!("SIDEVOICE_BUILD_DATE"),
    )
}

/// Everything under one lock, changed together; the network is reached outside it.
#[derive(Default)]
struct Updates {
    status: Status,
    /// The update being offered: found (available), downloading, or downloaded (ready, with its bytes).
    update: Option<Update>,
    bytes: Option<Vec<u8>>,
    /// An install is under way (a restart's, or quit's): one at a time, and quitting waits for it.
    installing: bool,
}

type Updater = Mutex<Updates>;

/// Whether this copy updates itself; found once (and logged with `SIDEVOICE_DEBUG=1`).
pub fn offered() -> bool {
    static OFFERED: OnceLock<bool> = OnceLock::new();
    *OFFERED.get_or_init(|| {
        let release = Channel::parse(env!("SIDEVOICE_BUILD_CHANNEL")) == Some(Channel::Release);
        let offered = release && replaceable();
        crate::debug(&format!("updater on: {offered} (a release: {release})"));
        offered
    })
}

/// Whether this copy can replace itself: a release's .app run from its bundle, not from a read-only volume (the .dmg,
/// a translocated copy); its AppImage run as one; its Windows installer's install. Never a .deb or a bare binary.
fn replaceable() -> bool {
    use tauri::utils::config::BundleType;
    match tauri::utils::platform::bundle_type() {
        Some(BundleType::App) => {
            std::env::current_exe().is_ok_and(|exe| core::in_app_bundle(&exe) && !on_read_only_volume(&exe))
        }
        Some(BundleType::AppImage) => std::env::var_os("APPIMAGE").is_some(),
        Some(BundleType::Nsis) => true,
        _ => false,
    }
}

/// Whether the folder holding the `.app` is on a read-only volume.
fn on_read_only_volume(exe: &std::path::Path) -> bool {
    let Some(folder) = exe.ancestors().nth(4) else { return false };
    let probe = folder.join(format!(".sidevoice-update-check-{}", std::process::id()));
    let opened = std::fs::OpenOptions::new().write(true).create_new(true).open(&probe);
    let _ = std::fs::remove_file(&probe);
    opened.is_err_and(|e| e.kind() == std::io::ErrorKind::ReadOnlyFilesystem)
}

/// At start, where this copy updates itself: the plugin, and the checks.
pub fn setup(app: &AppHandle) {
    if !offered() {
        return;
    }
    if let Err(e) = app.plugin(tauri_plugin_updater::Builder::new().build()) {
        eprintln!("sidevoice: the updater did not start: {e}");
        return;
    }
    app.manage(Updater::default());
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

/// Changes the updates under their lock and, still under it (so the page hears changes in order), tells the page the
/// status when it changed: `window.__sidevoiceDesktop.updaterEvent(status)`.
fn with<T>(app: &AppHandle, change: impl FnOnce(&mut Updates) -> T) -> T {
    let updater = app.state::<Updater>();
    let mut updates = updater.lock().unwrap();
    let before = updates.status.clone();
    let answer = change(&mut updates);
    if updates.status != before {
        if let (Some(window), Ok(json)) = (crate::room_window(app), serde_json::to_string(&updates.status)) {
            let _ = window.eval(format!("window.__sidevoiceDesktop && window.__sidevoiceDesktop.updaterEvent({json})"));
        }
    }
    answer
}

/// Checks the releases. A newer version found is `available` (and in automatic mode downloaded at once); one already
/// downloading or ready is left as it is.
async fn check(app: &AppHandle) {
    let checked = async { app.updater_builder().timeout(CHECK_FOR).build()?.check().await }.await;
    with(app, |updates| match checked {
        Ok(found) => {
            updates.status.error = None;
            if matches!(updates.status.state, State::Idle | State::Available) {
                updates.status =
                    found.as_ref().map_or_else(Status::default, |u| Status::new(State::Available, &u.version));
                updates.update = found;
            }
        }
        Err(e) => {
            crate::debug(&format!("updater check failed: {e}"));
            updates.status = updates.status.clone().failed(ErrorCode::CheckFailed);
        }
    });
    if mode(app) == Mode::Automatic {
        download(app);
    }
}

/// Starts downloading the update available: `false` when there is none (one downloading or ready is fine).
fn download(app: &AppHandle) -> bool {
    let claimed = with(app, |updates| match (updates.status.state, &updates.update) {
        (State::Available, Some(update)) => {
            updates.status = Status::new(State::Downloading, &update.version);
            Ok(update.clone())
        }
        (state, _) => Err(state != State::Idle),
    });
    let update = match claimed {
        Ok(update) => update,
        Err(fine) => return fine,
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut received = 0u64;
        let progress = |chunk: usize, total: Option<u64>| {
            received += chunk as u64;
            with(&app, |updates| updates.status.progress = core::progress(received, total));
        };
        let downloaded = tokio::time::timeout(DOWNLOAD_FOR, update.download(progress, || {})).await;
        with(&app, |updates| match downloaded {
            Ok(Ok(bytes)) => {
                updates.status = Status::new(State::Ready, &update.version);
                updates.bytes = Some(bytes);
            }
            failed => {
                crate::debug(&format!("updater download of {} failed: {failed:?}", update.version));
                updates.status = Status::new(State::Available, &update.version).failed(ErrorCode::DownloadFailed);
            }
        });
    });
    true
}

/// Installs the update ready, unless one is installing already: `restart`, then restarts into it (Windows: the plugin
/// exits and the installer starts the app again only then); otherwise the app is quitting. The local host stops only
/// once the install succeeded.
fn install(app: &AppHandle, restart: bool) {
    let claimed = with(app, |updates| match (&updates.update, &updates.bytes) {
        (Some(update), Some(bytes)) if !updates.installing && updates.status.state == State::Ready => {
            updates.installing = true;
            Some((update.clone(), bytes.clone()))
        }
        _ => None,
    });
    let Some((update, bytes)) = claimed else { return };
    match update.restart_after_install(restart).install(&bytes) {
        Ok(()) if restart => {
            crate::local_host::shutdown(app);
            app.restart()
        }
        Ok(()) => {}
        Err(e) => {
            crate::debug(&format!("updater install failed: {e}"));
            with(app, |updates| {
                updates.installing = false;
                updates.status = updates.status.clone().failed(ErrorCode::InstallFailed);
            });
        }
    }
}

/// The app is quitting (`RunEvent::Exit`): an install under way (a restart's) is waited for; in automatic mode an
/// update ready is installed on the way out.
pub fn quit(app: &AppHandle) {
    if app.try_state::<Updater>().is_none() {
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(5 * 60);
    while with(app, |updates| updates.installing) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    if mode(app) == Mode::Automatic {
        install(app, false);
    }
}

fn refusal(key: &str, message: &str) -> Value {
    json!({ "key": key, "message": message })
}

fn unavailable() -> Value {
    refusal("update_unavailable", "There is no update to install.")
}

/// The commands answer only the current room window on the app's own page, and only where the updater runs.
fn caller(app: &AppHandle, webview: &Webview) -> Result<(), Value> {
    crate::room_page(app, webview).map_err(|why| refusal("bad_request", &why))?;
    app.try_state::<Updater>().map(|_| ()).ok_or_else(unavailable)
}

fn status(app: &AppHandle) -> Status {
    with(app, |updates| updates.status.clone())
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
    let dir = app.path().app_config_dir().map_err(|e| refusal("internal", &e.to_string()))?;
    {
        let state = app.state::<AppState>();
        let mut stored = state.settings.lock().unwrap();
        let changed = Settings { update_mode: mode, ..stored.clone().unwrap_or_else(Settings::first_run) };
        settings::save(&dir, &changed).map_err(|e| refusal("internal", &format!("not saved: {e}")))?;
        *stored = Some(changed);
    }
    if mode == Mode::Automatic {
        download(&app);
    }
    Ok(json!({ "mode": mode }))
}

#[tauri::command]
pub fn updater_status(app: AppHandle, webview: Webview) -> Result<Status, Value> {
    caller(&app, &webview)?;
    Ok(status(&app))
}

/// "Check for updates": answers the status once the check is done.
#[tauri::command]
pub async fn updater_check(app: AppHandle, webview: Webview) -> Result<Status, Value> {
    caller(&app, &webview)?;
    check(&app).await;
    Ok(status(&app))
}

/// Starts downloading the update found; refuses `update_unavailable` when there is none.
#[tauri::command]
pub fn updater_download(app: AppHandle, webview: Webview) -> Result<Status, Value> {
    caller(&app, &webview)?;
    if !download(&app) {
        return Err(unavailable());
    }
    Ok(status(&app))
}

/// "Restart to update": installs the ready update and restarts now; refuses `update_unavailable` unless one is ready.
#[tauri::command]
pub fn updater_restart(app: AppHandle, webview: Webview) -> Result<(), Value> {
    caller(&app, &webview)?;
    if status(&app).state != State::Ready {
        return Err(unavailable());
    }
    tauri::async_runtime::spawn_blocking(move || install(&app, true));
    Ok(())
}
