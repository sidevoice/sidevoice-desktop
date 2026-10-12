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
            Install::MacApp { in_bundle: std::env::current_exe().is_ok_and(|exe| plan::in_app_bundle(&exe)) }
        }
        Some(BundleType::AppImage) => Install::AppImage { mounted: std::env::var_os("APPIMAGE").is_some() },
        Some(BundleType::Nsis) => Install::Nsis,
        Some(BundleType::Deb) => Install::Deb,
        Some(BundleType::Rpm | BundleType::Msi) => Install::Other,
        None => Install::Unbundled,
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
    let result = match app.updater() {
        Ok(updater) => updater.check().await.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    let found = match result {
        Ok(update) => {
            crate::debug(&format!("updater checked: {:?}", update.as_ref().map(|u| &u.version)));
            let version = update.as_ref().map(|u| u.version.clone());
            *state.announced.lock().unwrap() = update;
            Ok(version)
        }
        Err(e) => {
            // No release with a latest.json yet, no network, a manifest without this platform: nothing to offer.
            crate::debug(&format!("updater check failed: {e}"));
            Err(())
        }
    };
    let step = state.plan.lock().unwrap().checked(found);
    run(app, step);
}

/// Does what core `updater` decided, and tells the page when its status changed.
fn run(app: &AppHandle, step: Step) {
    notify(app);
    match step {
        Step::Nothing => {}
        Step::Download(version) => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { download(&app, version).await });
        }
        Step::Install => {
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || install_and_restart(&app));
        }
    }
}

async fn download(app: &AppHandle, version: String) {
    let Some(state) = app.try_state::<UpdaterState>() else { return };
    let update = state.announced.lock().unwrap().clone().filter(|u| u.version == version);
    let ok = match update {
        // Downloaded whole, then verified against the app's public key before anything else sees it.
        Some(update) => match update.download(|_, _| {}, || {}).await {
            Ok(bytes) => {
                crate::debug(&format!("updater downloaded {version} ({} bytes)", bytes.len()));
                *state.ready.lock().unwrap() = Some((update, bytes));
                true
            }
            Err(e) => {
                crate::debug(&format!("updater download of {version} failed: {e}"));
                false
            }
        },
        None => false,
    };
    let step = state.plan.lock().unwrap().downloaded(&version, ok);
    run(app, step);
}

/// Installs the update ready and restarts into it. Windows: the installer the update is takes over (the plugin exits
/// the app and the installer starts it again).
fn install_and_restart(app: &AppHandle) {
    let Some(state) = app.try_state::<UpdaterState>() else { return };
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
    let Some(state) = app.try_state::<UpdaterState>() else { return };
    // Read under the plan's lock, so two changes close together are applied in the order they were read.
    let step = {
        let mut plan = state.plan.lock().unwrap();
        plan.call(crate::in_call(app))
    };
    if step != Step::Nothing {
        run(app, step);
    }
}

/// The settings window turned automatic download on or off.
pub fn set_auto_download(app: &AppHandle, on: bool) {
    let Some(state) = app.try_state::<UpdaterState>() else { return };
    let step = state.plan.lock().unwrap().set_auto_download(on);
    run(app, step);
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
    let state = app.try_state::<UpdaterState>().ok_or_else(unavailable)?;
    // The plan knows the call as it is now (`call_changed`): a restart asked for during one waits for it to end.
    let step = state.plan.lock().unwrap().restart().map_err(|plan::NothingToInstall| unavailable())?;
    crate::debug(&format!("updater restart asked: {step:?}"));
    run(&app, step);
    Ok(())
}
