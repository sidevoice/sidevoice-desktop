//! The app's updates (docs/BRIDGE.md → "The updater"): `window.__sidevoiceDesktop.host.updater`, a thin layer over
//! tauri-plugin-updater (from Rust only: the page is granted none of its own commands). The plugin reads the release's
//! `latest.json` (tauri.conf.json `plugins.updater`), downloads and verifies the update, and installs it; this keeps
//! the mode, the schedule (at launch, then every 6 hours), the status the page reads, and install-on-quit.
//!
//! Only a numbered release has it (`SIDEVOICE_BUILD_CHANNEL`, build.rs; RELEASING.md → "Channels"), and only where
//! that copy can replace itself ([`offered`]). Elsewhere nothing is checked and the page has no `host.updater`.

use serde_json::{json, Value};
use sidevoice_desktop_core::build_info::{BuildInfo, Channel};
use sidevoice_desktop_core::settings::{self, Settings};
use sidevoice_desktop_core::updater::{self as core, ErrorCode, Mode, State, Status};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Manager, Webview};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::AppState;

/// Checked at launch and then this often, by the wall clock (a computer that slept through it checks on waking).
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const TICK: Duration = Duration::from_secs(10 * 60);
/// A check or a download that hangs is given up (the plugin's request timeout; the next check tries again).
const TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// `host.build`: the channel, the app's version (a nightly's is the last release's), and a nightly's commit and date.
pub fn build(app: &AppHandle) -> BuildInfo {
    BuildInfo::new(
        env!("SIDEVOICE_BUILD_CHANNEL"),
        &app.package_info().version.to_string(),
        env!("SIDEVOICE_BUILD_COMMIT"),
        env!("SIDEVOICE_BUILD_DATE"),
    )
}

#[derive(Default)]
struct Updater {
    status: Mutex<Status>,
    /// The update the last check found, until it is downloaded.
    found: Mutex<Option<Update>>,
    /// The update downloaded and verified, with its bytes.
    ready: Mutex<Option<(Update, Vec<u8>)>>,
}

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

/// Whether this copy can replace itself: a release's .app not on a read-only volume (the .dmg, a translocated copy),
/// its AppImage run as one, its Windows installer's install. Never a .deb (the package manager's) or a bare binary.
fn replaceable() -> bool {
    use tauri::utils::config::BundleType;
    match tauri::utils::platform::bundle_type() {
        Some(BundleType::App) => std::env::current_exe().is_ok_and(|exe| !on_read_only_volume(&exe)),
        Some(BundleType::AppImage) => std::env::var_os("APPIMAGE").is_some(),
        Some(BundleType::Nsis) => true,
        _ => false,
    }
}

/// Whether the folder holding the `.app` is on a read-only volume (the mounted .dmg it was opened from).
fn on_read_only_volume(exe: &std::path::Path) -> bool {
    let Some(folder) = exe.ancestors().nth(4) else { return false };
    let probe = folder.join(format!(".sidevoice-update-check-{}", std::process::id()));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            false
        }
        Err(e) => e.kind() == std::io::ErrorKind::ReadOnlyFilesystem,
    }
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
            let started = SystemTime::now();
            check(&app).await;
            while started.elapsed().is_ok_and(|passed| passed < EVERY) {
                tokio::time::sleep(TICK).await;
            }
        }
    });
}

fn mode(app: &AppHandle) -> Mode {
    app.state::<AppState>().settings.lock().unwrap().as_ref().map(|s| s.update_mode).unwrap_or_default()
}

/// Changes the status and tells the page when it changed: `window.__sidevoiceDesktop.updaterEvent(status)`.
fn set(app: &AppHandle, change: impl FnOnce(&mut Status)) -> Status {
    let updater = app.state::<Updater>();
    let (before, after) = {
        let mut status = updater.status.lock().unwrap();
        let before = status.clone();
        change(&mut status);
        (before, status.clone())
    };
    if before != after {
        if let (Some(window), Ok(json)) = (crate::room_window(app), serde_json::to_string(&after)) {
            let _ = window.eval(format!("window.__sidevoiceDesktop && window.__sidevoiceDesktop.updaterEvent({json})"));
        }
    }
    after
}

/// Checks the releases. A newer version found is `available`, and in automatic mode
/// downloaded at once; one already downloading or ready is left as it is.
async fn check(app: &AppHandle) {
    let updater = app.state::<Updater>();
    let checked = match app.updater_builder().timeout(TIMEOUT).build() {
        Ok(plugin) => plugin.check().await,
        Err(e) => Err(e),
    };
    match checked {
        Ok(found) => {
            crate::debug(&format!("updater checked: {:?}", found.as_ref().map(|u| &u.version)));
            set(app, |status| {
                status.error = None;
                if matches!(status.state, State::Idle | State::Available) {
                    *status = match &found {
                        Some(update) => Status::new(State::Available, &update.version),
                        None => Status::default(),
                    };
                }
            });
            *updater.found.lock().unwrap() = found;
            if mode(app) == Mode::Automatic {
                let _ = download(app);
            }
        }
        Err(e) => {
            crate::debug(&format!("updater check failed: {e}"));
            set(app, |status| *status = status.clone().failed(ErrorCode::CheckFailed));
        }
    }
}

/// Starts downloading the update found: `false` when none is available (one downloading or ready is fine).
fn download(app: &AppHandle) -> bool {
    let Some(update) = app.state::<Updater>().found.lock().unwrap().clone() else { return false };
    let mut state = State::Idle;
    set(app, |status| {
        state = status.state;
        if state == State::Available {
            *status = Status::new(State::Downloading, &update.version);
        }
    });
    if state != State::Available {
        return state != State::Idle;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut received = 0u64;
        let progress = |chunk: usize, total: Option<u64>| {
            received += chunk as u64;
            set(&app, |status| status.progress = core::progress(received, total));
        };
        match update.download(progress, || {}).await {
            Ok(bytes) => {
                crate::debug(&format!("updater downloaded {} ({} bytes)", update.version, bytes.len()));
                let version = update.version.clone();
                *app.state::<Updater>().ready.lock().unwrap() = Some((update, bytes));
                set(&app, |status| *status = Status::new(State::Ready, &version));
            }
            Err(e) => {
                crate::debug(&format!("updater download of {} failed: {e}", update.version));
                let failed = Status::new(State::Available, &update.version).failed(ErrorCode::DownloadFailed);
                set(&app, |status| *status = failed);
            }
        }
    });
    true
}

/// Installs the update ready; `restart`: then restarts into it (Windows: the plugin exits and the installer starts the
/// app again only then). Taken while it installs, so the exit of a restart does not install it a second time.
fn install(app: &AppHandle, restart: bool) {
    let updater = app.state::<Updater>();
    let Some((update, bytes)) = updater.ready.lock().unwrap().take() else { return };
    crate::debug(&format!("updater installing {} (restart: {restart})", update.version));
    if restart {
        crate::local_host::shutdown(app);
    }
    match update.clone().restart_after_install(restart).install(&bytes) {
        Ok(()) if restart => app.restart(),
        Ok(()) => {}
        Err(e) => {
            crate::debug(&format!("updater install failed: {e}"));
            *updater.ready.lock().unwrap() = Some((update, bytes));
            set(app, |status| *status = status.clone().failed(ErrorCode::InstallFailed));
        }
    }
}

/// The app is quitting (`RunEvent::Exit`): in automatic mode, an update ready is installed on the way out.
pub fn quit(app: &AppHandle) {
    if app.try_state::<Updater>().is_some() && mode(app) == Mode::Automatic {
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
    app.state::<Updater>().status.lock().unwrap().clone()
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
    if app.state::<Updater>().ready.lock().unwrap().is_none() {
        return Err(unavailable());
    }
    tauri::async_runtime::spawn_blocking(move || install(&app, true));
    Ok(())
}
