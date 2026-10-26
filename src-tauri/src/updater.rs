//! `host.updater` (docs/BRIDGE.md → "The updater"): schedule, status and install-on-quit over tauri-plugin-updater.

use serde_json::{json, Value};
use sidevoice_desktop_core::updater::{self as core, ErrorCode, Mode, State, Status};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, Webview};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::platform::update_eligibility::{NativeUpdateEligibility, UpdateEligibility};
use crate::AppState;

/// One lock over the status and its update; `installing`: an install under way, which quitting waits for; `task`: the
/// download running, which `cancel` aborts; `downloads`: which download may write and
/// the version the person stopped, kept for this run of the app.
#[derive(Default)]
struct Updates {
    status: Status,
    update: Option<Update>,
    bytes: Option<Vec<u8>>,
    installing: bool,
    task: Option<tauri::async_runtime::JoinHandle<()>>,
    downloads: core::Downloads,
}

/// How long a check waits for the feed; before an install, how long the last look at it may take (an offline restart
/// installs what it has).
const CHECK_TIMEOUT: Duration = Duration::from_secs(2 * 60);
const RECHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// The channel this build was built for (build.rs): `release`, `nightly` or `dev`.
const CHANNEL: &str = env!("SIDEVOICE_BUILD_CHANNEL");

/// This build's feed (core `feed`): a release's the releases', a nightly's the nightly's; a dev build none.
fn feed() -> Option<url::Url> {
    core::feed(CHANNEL)
}

pub fn setup(app: &AppHandle) {
    if !NativeUpdateEligibility::available(CHANNEL) {
        return;
    }
    if let Err(e) = app.plugin(tauri_plugin_updater::Builder::new().build()) {
        return log::error!("the updater did not start: {e}");
    }
    app.manage(Mutex::new(Updates::default()));
    // What a relaunch after an update came up as: the version and the bundle it runs from.
    crate::debug(&format!(
        "updater: started {} ({CHANNEL}) from {:?}",
        app.package_info().version,
        std::env::current_exe().ok()
    ));
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
/// is left as it is, unless the feed has moved past it.
async fn check(app: &AppHandle) {
    look(app, CHECK_TIMEOUT).await;
    if current_mode(app) == Mode::Automatic {
        download(app, false);
    }
}

/// Reads the feed (waiting `timeout` at most) and records what it offers; a download or an update ready that it moved
/// past is dropped.
async fn look(app: &AppHandle, timeout: Duration) {
    let checked = async {
        let updater = app.updater_builder().endpoints(feed().into_iter().collect())?;
        updater.timeout(timeout).build()?.check().await
    }
    .await;
    let running = app.package_info().version.to_string();
    with(app, |u| match checked {
        Ok(found) => {
            crate::debug(&format!(
                "updater: running {running} ({CHANNEL}), the feed offers {}, status {:?}",
                found.as_ref().map_or("nothing newer", |f| f.version.as_str()),
                u.status.state
            ));
            // The feed moved on while an older one was downloading or ready (a nightly comes with every merge):
            // that one is dropped, or the app would install it and be offered the newer one again after restarting.
            if let Some(found) = found.as_ref().filter(|_| !u.installing) {
                let (state, version) = (u.status.state, u.status.version.clone());
                if core::drop_stale(&mut u.status, &mut u.downloads, &found.version) {
                    crate::debug(&format!(
                        "updater: discarding {state:?} of {version:?}, the feed offers {}",
                        found.version
                    ));
                    if let Some(task) = u.task.take() {
                        task.abort();
                    }
                    u.bytes = None;
                }
            }
            if matches!(u.status.state, State::Idle | State::Available) {
                u.status = found.as_ref().map_or_else(Status::default, |f| Status::new(State::Available, &f.version));
                u.update = found;
            } else {
                u.status.error = None;
            }
        }
        Err(e) => {
            crate::debug(&format!("updater check failed: {e}"));
            u.status = u.status.clone().failed(ErrorCode::CheckFailed);
        }
    });
}

/// Starts downloading the update known. `asked`: the person asked (`download`), which also lifts a cancel; otherwise
/// (automatic mode) a version the person cancelled is left alone.
fn download(app: &AppHandle, asked: bool) {
    let Some(permit) = crate::data::admit(app) else { return };
    with(app, |u| {
        let mut update = u.update.clone().filter(|_| u.status.state == State::Available)?;
        let token = u.downloads.start(&update.version, asked)?;
        u.status = Status::new(State::Downloading, &update.version);
        crate::debug(&format!("updater: downloading {}", update.version));
        update.timeout = Some(Duration::from_secs(60 * 60));
        // Spawned under the lock, so `cancel` finds the task whenever the status says downloading.
        u.task = Some(tauri::async_runtime::spawn(download_task(app.clone(), update, token, permit)));
        Some(())
    });
}

/// The download itself, in memory (the plugin hands back the bytes; there is no partial file). Aborting only stops it
/// at its next await, so what it writes (progress, bytes, result) is accepted only while its `token` is the current
/// one: a cancel, and any download after it, make a late write a no-op.
async fn download_task(app: AppHandle, update: Update, token: core::Token, _permit: crate::data::Permit) {
    let mut received = 0;
    let progress = |chunk: usize, total: Option<u64>| {
        received += chunk as u64;
        with(&app, |u| {
            if u.downloads.owns(token) {
                u.status.progress = core::progress(received, total);
            }
        });
    };
    let downloaded = update.download(progress, || {}).await;
    with(&app, |u| {
        if !u.downloads.owns(token) {
            return;
        }
        u.task = None;
        match downloaded {
            Ok(bytes) => {
                crate::debug(&format!("updater: {} is ready ({} bytes)", update.version, bytes.len()));
                (u.status, u.bytes) = (Status::new(State::Ready, &update.version), Some(bytes));
            }
            Err(e) => {
                crate::debug(&format!("updater download failed: {e}"));
                u.status = Status::new(State::Available, &update.version).failed(ErrorCode::DownloadFailed);
            }
        }
    });
}

/// Stops the download running, dropping what it had received; the update stays known, `available`. With none running,
/// nothing happens.
fn cancel(app: &AppHandle) {
    with(app, |u| {
        if let Some(status) = u.downloads.cancel(&u.status) {
            if let Some(task) = u.task.take() {
                task.abort();
            }
            u.status = status;
        }
    });
}

/// Whoever takes the bytes installs; the local host stops only once the install succeeded.
fn install(app: &AppHandle, restart: bool) {
    let Some(_permit) = crate::data::admit(app) else { return };
    let claimed = with(app, |u| u.update.clone().zip(u.bytes.take()).inspect(|_| u.installing = true));
    let Some((update, bytes)) = claimed else { return };
    crate::debug(&format!(
        "updater: installing {} over {} ({} bytes, restart: {restart}) at {:?}",
        update.version,
        update.current_version,
        bytes.len(),
        std::env::current_exe().ok()
    ));
    let installed = update.restart_after_install(restart).install(&bytes);
    with(app, |u| {
        u.installing = false;
        if let Err(e) = &installed {
            crate::debug(&format!("updater install failed: {e}"));
            (u.bytes, u.status) = (Some(bytes), u.status.clone().failed(ErrorCode::InstallFailed));
        }
    });
    if installed.is_ok() && restart {
        crate::debug("updater: installed, restarting");
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
    while installing(app) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    if current_mode(app) == Mode::Automatic {
        // Only if what is ready is still the newest: a last look at the feed (from this thread with no runtime
        // context, as `voice` stops its calls), which drops it otherwise.
        if with(app, |u| u.bytes.is_some()) {
            crate::debug("updater: looking at the feed before installing on quit");
            tauri::async_runtime::block_on(look(app, RECHECK_TIMEOUT));
        }
        install(app, false);
    }
}

pub fn installing(app: &AppHandle) -> bool {
    app.try_state::<Mutex<Updates>>().is_some_and(|_| with(app, |u| u.installing))
}

fn unavailable() -> Value {
    json!({ "key": "update_unavailable", "message": "There is no update to install." })
}

/// `host.updater`'s one command: `action` is settings, set-mode (with `mode`), status, check, download, cancel or restart.
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
            download(&app, true);
            Some(status(&app)).filter(|s| s.state != State::Idle).map(|s| json!(s)).ok_or_else(unavailable)
        }
        ("cancel", _) => {
            cancel(&app);
            Ok(Value::Null)
        }
        ("restart", _) if status(&app).state == State::Ready => {
            // The feed may have moved on since the download: that one is then dropped and the newer one downloaded
            // (shown to the page as the status), never installed.
            crate::debug("updater: looking at the feed before restarting");
            look(&app, RECHECK_TIMEOUT).await;
            if status(&app).state != State::Ready {
                if current_mode(&app) == Mode::Automatic {
                    download(&app, false);
                }
                return Ok(Value::Null);
            }
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
        download(app, false);
    }
    Ok(())
}
