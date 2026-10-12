//! The app's updates (docs/BRIDGE.md → "The updater"), as the room page reaches them:
//! `window.__sidevoiceDesktop.host.updater`.
//!
//! tauri-plugin-updater, from Rust only (the page is granted none of its own commands): it reads the release's
//! `latest.json` (the endpoint and the public key are `plugins.updater` in tauri.conf.json), downloads the update and
//! verifies its signature, and installs it. What to do when is core `updater`'s: checked at launch and every 6 hours,
//! downloaded in the background unless the person turned that off (the settings window), installed on the person's
//! "Restart to update" — once the call has ended when there is one (the room's call joined, or the voice call the app
//! runs listening).
//!
//! Only where this copy can update itself ([`offered`]: a release's .app, AppImage or Windows installer). Elsewhere —
//! a .deb, a dev build — the page has no `host.updater`, nothing is checked, `updater_status` says nothing is
//! available and `updater_restart` refuses.

use serde_json::{json, Value};
use sidevoice_desktop_core::updater::{self as plan, Install, Status, Step, Updates};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Manager, Webview};
use tauri_plugin_updater::{Update, UpdaterExt};

/// How often the releases are checked after the first check, at launch.
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// How long a check may take: one that hangs would hold every later check back.
const CHECK_FOR: Duration = Duration::from_secs(2 * 60);
/// How long a download may take, so one that stalls is given up and tried again at the next check.
const DOWNLOAD_FOR: Duration = Duration::from_secs(60 * 60);

pub struct UpdaterState {
    plan: Mutex<Updates>,
    /// The update the last check announced.
    announced: Mutex<Option<Update>>,
    /// The update downloaded and verified, with its bytes.
    ready: Mutex<Option<(Update, Vec<u8>)>>,
}

/// Whether this copy of the app can update itself; found once, at start (and logged with `SIDEVOICE_DEBUG=1`).
pub fn offered() -> bool {
    static OFFERED: OnceLock<bool> = OnceLock::new();
    *OFFERED.get_or_init(|| match plan::offered(cfg!(debug_assertions), install()) {
        Ok(()) => true,
        Err(why) => {
            crate::debug(&format!("updater off: {why}"));
            false
        }
    })
}

/// How this copy was installed: the bundle Tauri's bundler wrote into the binary, and where it runs from.
fn install() -> Install {
    use tauri::utils::config::BundleType;
    match tauri::utils::platform::bundle_type() {
        Some(BundleType::App | BundleType::Dmg) => {
            let exe = std::env::current_exe().ok();
            Install::MacApp {
                in_bundle: exe.as_deref().is_some_and(plan::in_app_bundle),
                read_only: exe.as_deref().is_some_and(on_read_only_volume),
            }
        }
        Some(BundleType::AppImage) => Install::AppImage { mounted: std::env::var_os("APPIMAGE").is_some() },
        Some(BundleType::Nsis) => Install::Nsis,
        Some(BundleType::Deb) => Install::Deb,
        Some(BundleType::Rpm | BundleType::Msi) => Install::Other,
        None => Install::Unbundled,
    }
}

/// Whether the `.app` holding `exe` is on a volume that cannot be written (the mounted .dmg it was opened from): a file
/// next to it cannot be made for that reason. Any other refusal (a folder only an administrator writes) is not: the
/// plugin asks for the administrator's password when it installs there.
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

/// At start: the plugin and the checks, where this copy can update itself. Without the plugin the app still runs: its
/// page is then told nothing is available.
pub fn setup(app: &AppHandle, auto_download: bool) {
    if !offered() {
        return;
    }
    if let Err(e) = app.plugin(tauri_plugin_updater::Builder::new().build()) {
        eprintln!("sidevoice: the updater did not start: {e}");
        return;
    }
    app.manage(UpdaterState {
        plan: Mutex::new(Updates::new(auto_download)),
        announced: Mutex::default(),
        ready: Mutex::default(),
    });
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            check(&app).await;
            tokio::time::sleep(EVERY).await;
        }
    });
}

async fn check(app: &AppHandle) {
    let Some(state) = app.try_state::<UpdaterState>() else { return };
    if !state.plan.lock().unwrap().begin_check() {
        return;
    }
    let result = match updater(app) {
        Ok(updater) => match tokio::time::timeout(CHECK_FOR, updater.check()).await {
            Ok(checked) => checked.map_err(|e| e.to_string()),
            Err(_) => Err("timed out".into()),
        },
        Err(e) => Err(e),
    };
    let checked = match &result {
        Ok(update) => {
            crate::debug(&format!("updater checked: {:?}", update.as_ref().map(|u| &u.version)));
            Ok(update.as_ref().map(|u| u.version.clone()))
        }
        Err(e) => {
            // No release with a latest.json yet, no network, a manifest without this platform: nothing to offer.
            crate::debug(&format!("updater check failed: {e}"));
            Err(())
        }
    };
    transition(app, |plan| {
        // What was announced changes with the plan, under its lock: a download it decides takes that update.
        if let Ok(update) = result {
            *state.announced.lock().unwrap() = update;
        }
        plan.checked(checked)
    });
}

/// The plugin's updater: the releases' endpoint (tauri.conf.json), or in CI's probe build an address that never
/// answers, so the probe never depends on what the releases hold, nor installs one.
fn updater(app: &AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    let builder = app.updater_builder();
    #[cfg(feature = "probe")]
    let builder = builder
        .endpoints(vec!["https://127.0.0.1:9/latest.json".parse().expect("a URL")])
        .map_err(|e| e.to_string())?;
    builder.build().map_err(|e| e.to_string())
}

/// Changes the plan under its lock and does what it decided; tells the page its status may have changed. A download
/// takes the update announced as it was decided, never one a later check put in its place.
fn transition(app: &AppHandle, change: impl FnOnce(&mut Updates) -> Step) {
    let Some(state) = app.try_state::<UpdaterState>() else { return };
    let (step, update) = {
        let mut plan = state.plan.lock().unwrap();
        let step = change(&mut plan);
        let update = match &step {
            Step::Download(version) => state.announced.lock().unwrap().clone().filter(|u| &u.version == version),
            _ => None,
        };
        (step, update)
    };
    notify(app);
    match step {
        Step::Nothing => {}
        Step::Download(version) => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { download(&app, version, update).await });
        }
        Step::Install => {
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || install_and_restart(&app));
        }
    }
}

async fn download(app: &AppHandle, version: String, update: Option<Update>) {
    let Some(state) = app.try_state::<UpdaterState>() else { return };
    let ok = match update {
        // Downloaded whole, then verified against the app's public key before anything else sees it.
        Some(update) => match tokio::time::timeout(DOWNLOAD_FOR, update.download(|_, _| {}, || {})).await {
            Ok(Ok(bytes)) => {
                crate::debug(&format!("updater downloaded {version} ({} bytes)", bytes.len()));
                *state.ready.lock().unwrap() = Some((update, bytes));
                true
            }
            Ok(Err(e)) => {
                crate::debug(&format!("updater download of {version} failed: {e}"));
                false
            }
            Err(_) => {
                crate::debug(&format!("updater download of {version} timed out"));
                false
            }
        },
        None => false,
    };
    transition(app, |plan| plan.downloaded(&version, ok));
}

/// Installs the update ready and restarts into it, unless a call began as the install was decided: then it waits for
/// that call to end. Windows: the installer the update is takes over (the plugin exits the app and the installer
/// starts it again).
fn install_and_restart(app: &AppHandle) {
    let Some(state) = app.try_state::<UpdaterState>() else { return };
    {
        // Under the plan's lock, as `call_changed` reads the call: a call that begins after this waits for the restart.
        let mut plan = state.plan.lock().unwrap();
        if crate::in_call(app) {
            crate::debug("updater install waits: a call began");
            plan.install_deferred();
            return;
        }
    }
    let ready = state.ready.lock().unwrap().take();
    let result = match &ready {
        Some((update, bytes)) => {
            crate::debug(&format!("updater installing {}", update.version));
            update.install(bytes).map_err(|e| e.to_string())
        }
        None => Err("nothing downloaded".into()),
    };
    match result {
        Ok(()) => {
            crate::local_host::shutdown(app);
            app.restart();
        }
        Err(e) => {
            crate::debug(&format!("updater install failed: {e}"));
            state.plan.lock().unwrap().install_failed();
            notify(app);
        }
    }
}

/// Tells the page to read the status again: `window.__sidevoiceDesktop.updaterEvent()`.
fn notify(app: &AppHandle) {
    if let Some(window) = crate::room_window(app) {
        let _ = window.eval("window.__sidevoiceDesktop && window.__sidevoiceDesktop.updaterEvent()");
    }
}

/// A call began or ended (the room's snapshot, the voice call the app runs): a restart the person asked for during it
/// happens once it has ended.
pub fn call_changed(app: &AppHandle) {
    // Read under the plan's lock, so two changes close together are applied in the order they were read.
    transition(app, |plan| plan.call(crate::in_call(app)));
}

/// The settings window turned automatic download on or off.
pub fn set_auto_download(app: &AppHandle, on: bool) {
    transition(app, |plan| plan.set_auto_download(on));
}

fn refusal(key: &str, message: &str) -> Value {
    json!({ "key": key, "message": message })
}

fn caller(app: &AppHandle, webview: &Webview) -> Result<(), Value> {
    crate::room_page(app, webview).map_err(|why| {
        crate::debug(&format!("updater refused: {why}"));
        refusal("bad_request", &why)
    })
}

/// `{available, version?}`: `available` once an update is downloaded and ready (automatic download off: once one is
/// found). Where this copy cannot update itself, never available.
#[tauri::command]
pub fn updater_status(app: AppHandle, webview: Webview) -> Result<Status, Value> {
    caller(&app, &webview)?;
    Ok(app.try_state::<UpdaterState>().map(|state| state.plan.lock().unwrap().status()).unwrap_or_default())
}

/// "Restart to update": installs and restarts now, or once the update is downloaded, or once the call has ended.
/// Refuses `update_unavailable` when there is nothing to install.
#[tauri::command]
pub fn updater_restart(app: AppHandle, webview: Webview) -> Result<(), Value> {
    caller(&app, &webview)?;
    let unavailable = || refusal("update_unavailable", "There is no update to install.");
    if app.try_state::<UpdaterState>().is_none() {
        return Err(unavailable());
    }
    // The plan knows the call as it is now (`call_changed`): a restart asked for during one waits for it to end.
    let mut refused = false;
    transition(&app, |plan| match plan.restart() {
        Ok(step) => {
            crate::debug(&format!("updater restart asked: {step:?}"));
            step
        }
        Err(plan::NothingToInstall) => {
            refused = true;
            Step::Nothing
        }
    });
    if refused {
        return Err(unavailable());
    }
    Ok(())
}
