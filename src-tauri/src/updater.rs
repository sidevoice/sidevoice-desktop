//! `host.updater` (docs/BRIDGE.md → "The updater"): schedule, status and install-on-quit over tauri-plugin-updater.

use serde_json::{json, Value};
use sidevoice_desktop_core::updater::{self as core, ErrorCode, Mode, State, Status};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, Webview};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::AppState;

/// One lock over the status and its update; `installing`: an install under way, which quitting waits for.
#[derive(Default)]
struct Updates {
    status: Status,
    update: Option<Update>,
    bytes: Option<Vec<u8>>,
    installing: bool,
}

/// The channel this build was built for (build.rs): `release`, `nightly` or `dev`.
const CHANNEL: &str = env!("SIDEVOICE_BUILD_CHANNEL");

/// This build's feed (core `feed`): a release's the releases', a nightly's the nightly's; a dev build none.
fn feed() -> Option<url::Url> {
    core::feed(CHANNEL)
}

/// A build with a feed (a release or a nightly) that can replace itself: its `.app` bundle on a writable volume,
/// AppImage or NSIS install.
pub fn offered() -> bool {
    use tauri::utils::{config::BundleType, platform::bundle_type};
    feed().is_some()
        && match bundle_type() {
            Some(BundleType::App) => std::env::current_exe()
                .is_ok_and(|exe| core::in_app_bundle(&exe) && exe.ancestors().nth(4).is_some_and(writable)),
            Some(BundleType::AppImage) => std::env::var_os("APPIMAGE").is_some(),
            Some(BundleType::Nsis) => true,
            _ => false,
        }
}

/// Not on a read-only volume (the .dmg, a translocated copy).
fn writable(folder: &std::path::Path) -> bool {
    let probe = folder.join(format!(".sidevoice-update-check-{}", std::process::id()));
    let made = std::fs::OpenOptions::new().write(true).create_new(true).open(&probe);
    let _ = std::fs::remove_file(&probe);
    !made.is_err_and(|e| e.kind() == std::io::ErrorKind::ReadOnlyFilesystem)
}

pub fn setup(app: &AppHandle) {
    if !offered() {
        return;
    }
    if let Err(e) = app.plugin(tauri_plugin_updater::Builder::new().build()) {
        return eprintln!("sidevoice: the updater did not start: {e}");
    }
    app.manage(Mutex::new(Updates::default()));
    let app = app.clone();
    // At launch, then as often as its channel says (core `check_every`).
    tauri::async_runtime::spawn(async move {
        loop {
            check(&app).await;
            tokio::time::sleep(core::check_every(CHANNEL)).await;
        }
    });
}

fn current_mode(app: &AppHandle) -> Mode {
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

/// A newer version on this build's feed is `available` (automatic mode: downloaded at once); one downloading or ready
/// is left as it is.
async fn check(app: &AppHandle) {
    let checked = async {
        let updater = app.updater_builder().endpoints(feed().into_iter().collect())?;
        updater.timeout(Duration::from_secs(2 * 60)).build()?.check().await
    }
    .await;
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
    if current_mode(app) == Mode::Automatic {
        download(app);
    }
}

fn download(app: &AppHandle) {
    let Some(permit) = crate::data::admit(app) else { return };
    let Some(mut update) = with(app, |u| {
        let update = u.update.clone().filter(|_| u.status.state == State::Available)?;
        u.status = Status::new(State::Downloading, &update.version);
        Some(update)
    }) else {
        return;
    };
    update.timeout = Some(Duration::from_secs(60 * 60));
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _permit = permit;
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

/// Whoever takes the bytes installs; the local host stops only once the install succeeded.
fn install(app: &AppHandle, restart: bool) {
    let Some(_permit) = crate::data::admit(app) else { return };
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
    if app.state::<AppState>().resetting.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    if app.try_state::<Mutex<Updates>>().is_none() {
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(5 * 60);
    while with(app, |u| u.installing) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    if current_mode(app) == Mode::Automatic {
        install(app, false);
    }
}

#[cfg(target_os = "macos")]
pub fn installing(app: &AppHandle) -> bool {
    app.try_state::<Mutex<Updates>>().is_some_and(|_| with(app, |u| u.installing))
}

fn unavailable() -> Value {
    json!({ "key": "update_unavailable", "message": "There is no update to install." })
}

/// `host.updater`'s one command: `action` is settings, set-mode (with `mode`), status, check, download or restart.
#[tauri::command]
pub async fn updater(app: AppHandle, webview: Webview, action: String, mode: Option<Mode>) -> Result<Value, Value> {
    crate::room_page(&app, &webview).map_err(|why| json!({ "key": "bad_request", "message": why }))?;
    if !matches!(action.as_str(), "settings" | "status")
        && app.state::<AppState>().resetting.load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err(json!({ "key": "reset.busy" }));
    }
    app.try_state::<Mutex<Updates>>().ok_or_else(unavailable)?;
    let status = |app: &AppHandle| with(app, |u| u.status.clone());
    match (action.as_str(), mode) {
        ("settings", _) => Ok(json!({ "mode": current_mode(&app) })),
        ("set-mode", Some(mode)) => set_mode(&app, mode).map(|()| json!({ "mode": mode })),
        ("status", _) => Ok(json!(status(&app))),
        ("check", _) => {
            check(&app).await;
            Ok(json!(status(&app)))
        }
        ("download", _) => {
            download(&app);
            Some(status(&app)).filter(|s| s.state != State::Idle).map(|s| json!(s)).ok_or_else(unavailable)
        }
        ("restart", _) if status(&app).state == State::Ready => {
            tauri::async_runtime::spawn_blocking(move || install(&app, true));
            Ok(Value::Null)
        }
        ("restart", _) => Err(unavailable()),
        _ => Err(json!({ "key": "bad_request", "message": format!("not an updater action: {action}") })),
    }
}

/// Keeps the mode with the app's settings; automatic starts downloading an update already found.
fn set_mode(app: &AppHandle, mode: Mode) -> Result<(), Value> {
    crate::change_settings(app, |s| s.update_mode = mode).map_err(|e| crate::refusal("internal", e))?;
    if mode == Mode::Automatic {
        download(app);
    }
    Ok(())
}
