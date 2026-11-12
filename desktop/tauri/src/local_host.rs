//! The local host (docs/LOCAL_HOST.md) as the room page reaches it: `window.__sidevoiceHost.host.localHost`.
//!
//! The work is the `sidevoice-local-host` crate's (the checks, the pairing over the core's local channel, the proxy,
//! the poll); this module starts it with the app's own paths, answers the page's commands, and stops it as the app
//! quits. It is the same on every OS: what differs (a Unix socket or a named pipe, a process group or a Job Object) is
//! the crate's `platform`. It starts with the app on every OS; if it could not (no data directory, the proxy did not
//! start), every command refuses `unsupported`, except `reveal-log`, which needs no local host (the app's own log).
//!
//! Only the room window's own page may call these (capabilities/room.json, and [`crate::room_page`] in each). A
//! refusal is `{key, message}`, as the native engine's (docs/BRIDGE.md).

use serde_json::{json, Value};
use sidevoice_local_host::host::{Action, Config, Installing, LocalHost};
use sidevoice_local_host::install;
use sidevoice_local_host::npm::{Https, Minor};
use sidevoice_local_host::paths::DataDirs;
use sidevoice_local_host::platform::computer_name::{ComputerName, SystemComputerName};
use sidevoice_local_host::update;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Manager, Webview};

/// A command's answer: a value, or a refusal `{key, message}`.
type Answer = Result<Value, Value>;

fn refusal(key: &str, message: &str) -> Value {
    json!({ "key": key, "message": message })
}

fn unsupported() -> Value {
    refusal("unsupported", "The local host did not start on this computer.")
}

fn caller(app: &AppHandle, webview: &Webview) -> Result<(), Value> {
    crate::room_page(app, webview).map_err(|why| {
        crate::debug(&format!("local_host refused: {why}"));
        refusal("not_room_page", &why)
    })
}

/// The running local host, managed by the app once [`setup`] started it.
struct Hosted(Arc<LocalHost>);

fn hosted(app: &AppHandle) -> Result<Arc<LocalHost>, Value> {
    app.try_state::<Hosted>().map(|hosted| hosted.0.clone()).ok_or_else(unsupported)
}

/// `{state, failure?, core?, product_version, service?, calls?, attempts?, limit?, reachable}`.
#[tauri::command]
pub fn local_host_state(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    state(&app)
}

/// The pairing the page uses for the local host (design §4.1), or `null` unless it is running.
#[tauri::command]
pub fn local_host_pairing(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    Ok(hosted(&app)?.pairing().unwrap_or(Value::Null))
}

/// `{bundled, installed, core_api, update, latest, progress}`: what the service is against the minor this app works
/// with (docs/LOCAL_HOST.md → Updates). The page's bridge adds `bridge`.
#[tauri::command]
pub fn local_host_version(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    Ok(hosted(&app)?.version())
}

/// `start`, `stop`, `restart`, `service-install`, `service-uninstall`, `service-update`, `start-at-login-on`,
/// `start-at-login-off` (the connector's CLI, or for `service-update` the install's own path), `reconnect`,
/// `reveal-log`: the state after it.
#[tauri::command]
pub async fn local_host_action(app: AppHandle, webview: Webview, action: String) -> Answer {
    caller(&app, &webview)?;
    let _permit = crate::data::admit(&app).ok_or_else(|| refusal("reset.busy", "Data reset is under way."))?;
    if app.state::<crate::AppState>().resetting.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(refusal("reset.busy", "Data reset is under way."));
    }
    tauri::async_runtime::spawn_blocking(move || match action.as_str() {
        "reveal-log" => reveal(&app),
        _ => act(&app, &action),
    })
    .await
    .map_err(|e| refusal("internal", &e.to_string()))?
}

/// `npx sidevoice pair-device --json`, on the person's click: `{code, expires_in, reach}`.
#[tauri::command]
pub async fn local_host_pairing_code(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    let _permit = crate::data::admit(&app).ok_or_else(|| refusal("reset.busy", "Data reset is under way."))?;
    tauri::async_runtime::spawn_blocking(move || hosted(&app)?.pairing_code().map_err(|r| json!(r)))
        .await
        .map_err(|e| refusal("internal", &e.to_string()))?
}

/// "Prepare this computer": installs Sidevoice from the npm registry as install `job` (the page's id for this call;
/// docs/LOCAL_HOST.md → Install), and answers the state after it, once the app is paired with the new core (or has
/// waited for it long enough); `{key, step?, params?, log_tail?, message}` when it fails. One at a time
/// (`install.busy`); its progress is `local_host_install_progress`, and `local_host_install_cancel` stops it until the
/// package's installer starts.
#[tauri::command]
pub async fn local_host_install(app: AppHandle, webview: Webview, job: String) -> Answer {
    caller(&app, &webview)?;
    let _permit = crate::data::admit(&app).ok_or_else(|| refusal("reset.busy", "Data reset is under way."))?;
    if app.state::<crate::AppState>().resetting.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(refusal("reset.busy", "Data reset is under way."));
    }
    // Begun here, before the blocking work: its progress and its cancel find it from the start. Dropped unrun, it
    // frees the slot.
    let begun = hosted(&app)?.begin_install(&job).map_err(|failure| json!(failure))?;
    tauri::async_runtime::spawn_blocking(move || run_install(begun))
        .await
        .map_err(|e| refusal("internal", &e.to_string()))?
}

/// `{step, done, total, cancellable}` of install `job` while it runs; `null` for another id or once it has ended.
#[tauri::command]
pub fn local_host_install_progress(app: AppHandle, webview: Webview, job: String) -> Answer {
    caller(&app, &webview)?;
    Ok(hosted(&app)?.install_progress(&job).map(|progress| json!(progress)).unwrap_or(Value::Null))
}

/// Asks install `job` to stop: true while that is honoured (download, verification, unpacking), false once the
/// package's installer runs, or for an id not under way.
#[tauri::command]
pub fn local_host_install_cancel(app: AppHandle, webview: Webview, job: String) -> Answer {
    caller(&app, &webview)?;
    Ok(json!(hosted(&app)?.cancel_install(&job)))
}

fn state(app: &AppHandle) -> Answer {
    Ok(json!(hosted(app)?.state()))
}

fn act(app: &AppHandle, name: &str) -> Answer {
    let host = hosted(app)?;
    let report = match name {
        "reconnect" => host.reconnect(),
        "service-update" => run_update(&host)?,
        other => {
            let action = Action::parse(other).ok_or_else(|| refusal("bad_request", "Unknown action."))?;
            host.act(action).map_err(|r| json!(r))?
        }
    };
    Ok(json!(report))
}

/// The whole install, blocking (on the async runtime's blocking pool: the HTTPS client runs a runtime of its own, made
/// and dropped here). The host logs how it went; a failure is logged here in full, at error level.
fn run_install(installing: Installing) -> Answer {
    let fetch = Https::new().map_err(|failure| {
        log::error!("local-host install could not start: {}", failure.log_report());
        json!(failure)
    })?;
    let job = install::loggable_job(installing.id()).to_string();
    installing.run(&fetch, &install::Options::standard()).map(|report| json!(report)).map_err(|failure| {
        log::error!("local-host install {job} failed: {}", failure.log_report());
        json!(failure)
    })
}

/// `localHost.update()`: the registry asked now and the service updated if it is behind the minor this app works with (the
/// same install the background update runs). Blocking, like the install; a failure is logged here in full.
fn run_update(host: &Arc<LocalHost>) -> Result<sidevoice_local_host::state::Report, Value> {
    let fetch = Https::new().map_err(|failure| {
        log::error!("local-host update could not start: {}", failure.log_report());
        json!(failure)
    })?;
    host.update(&fetch, &install::Options::standard()).map_err(|failure| {
        log::error!("local-host update failed: {}", failure.log_report());
        json!(failure)
    })
}

/// Select the existing app log, and the core's (else connector) log when there is a local host, in the native file
/// manager, without a shell. The app log needs no connector, so this runs wherever the app does: the answer is the
/// local host's state after it, `null` where there is none.
fn reveal(app: &AppHandle) -> Answer {
    use sidevoice_desktop_core::i18n::t;

    let host_log = hosted(app).ok().and_then(|host| host.log_path());
    let logs = logs_to_show(crate::app_log_file(app), host_log);
    let language = app.state::<crate::AppState>().language;
    if logs.is_empty() {
        return Err(refusal("log.missing", t(language, "local_host.log_missing")));
    }
    crate::platform::reveal::files(app, &logs).map_err(|error| {
        log::warn!("Could not reveal logs: {error}");
        refusal("reveal.failed", t(language, "local_host.reveal_failed"))
    })?;
    Ok(state(app).unwrap_or(Value::Null))
}

/// What "Ver registro" shows: the app's own log first, then the core's (else the connector's) one, each only if it is
/// a file. The app's is there with or without a connector.
fn logs_to_show(app: Option<PathBuf>, connector: Option<PathBuf>) -> Vec<PathBuf> {
    let is_file = |path: &PathBuf| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file());
    app.into_iter().chain(connector).filter(is_file).collect()
}

/// Where the bundled page declares the service minor it works with (sidevoice-web's `apps/web/public/service.pin.json`,
/// `{"minor": "0.20"}`; the page is served from `voice/`).
const SERVICE_PIN: &str = "voice/service.pin.json";

/// The service minor the bundled page pins. `None` when the bundle has no such file (a web release before it: the
/// bundle then answers with its index page, which is no pin) or it is not one.
fn pinned_minor(app: &AppHandle) -> Option<Minor> {
    update::pin(&app.asset_resolver().get(SERVICE_PIN.into())?.bytes)
}

/// At start: the proxy and the poll, for the app's lifetime, and the service kept at the newest patch of the minor the
/// bundled page pins.
pub fn setup(app: &AppHandle) {
    // `D`, by the rule every Sidevoice program shares (`SIDEVOICE_DATA_DIR`, else the OS standard place).
    let Some(dirs) = DataDirs::from_env() else {
        crate::debug("local-host off: no data directory");
        return;
    };
    let Ok(app_dir) = crate::data::desktop_dir(app) else { return };
    let pinned_minor = pinned_minor(app);
    crate::debug(&format!(
        "local-host service minor: {}",
        pinned_minor.map_or("none pinned".into(), |minor| minor.to_string())
    ));
    let config = Config { dirs, app_dir, name: SystemComputerName::read(), pinned_minor };
    match LocalHost::start(config, crate::debug) {
        Ok(host) => {
            host.watch();
            keep_current(app, &host);
            app.manage(Hosted(host));
        }
        Err(e) => crate::debug(&format!("local-host off: the proxy could not start: {e}")),
    }
}

/// The background update (`LocalHost::keep_current`) on a thread of its own: the registry at launch and every 6 h, the
/// service updated when it is behind, once no call is on. Not without a pin, and not while a data reset is under way (its
/// permit is what an update holds, as every action does). The HTTPS client is made on that thread: it runs a runtime.
fn keep_current(app: &AppHandle, host: &Arc<LocalHost>) {
    if host.pinned_minor().is_none() {
        return;
    }
    let (app, host) = (app.clone(), host.clone());
    let started = std::thread::Builder::new().name("local-host-update".into()).spawn(move || match Https::new() {
        Ok(fetch) => host.keep_current(&fetch, &install::Options::standard(), || crate::data::admit(&app)),
        Err(failure) => log::error!("local-host updates could not start: {}", failure.log_report()),
    });
    if let Err(e) = started {
        log::error!("local-host updates could not start: {e}");
    }
}

/// As the app quits: the poll and the proxy stop, every tunnel closes.
pub fn shutdown(app: &AppHandle) {
    if let Some(hosted) = app.try_state::<Hosted>() {
        hosted.0.shutdown();
    }
}

/// A connector install under way: the reset refuses before its confirmation. Never where there is no local host.
pub fn installing(app: &AppHandle) -> bool {
    hosted(app).is_ok_and(|host| json!(host.state())["state"] == "installing")
}

/// The connector's CLI as the reset runs it: the one the install in `D` recorded, checked as the local-host crate
/// checks it (`Cli::installed`). Nothing else runs the connector's `uninstall`.
pub use sidevoice_local_host::cli::Cli as ConnectorCli;

/// Why the reset has no connector CLI to run.
pub enum NoConnectorCli {
    /// The install record in `D` is missing or unsafe, or names nothing runnable.
    Refused,
}

/// The connector's CLI the install in `data` (`D`) recorded, for the reset's connector step.
pub fn connector_cli(data: &std::path::Path) -> Result<ConnectorCli, NoConnectorCli> {
    ConnectorCli::installed(&DataDirs::new(data)).map_err(|_| NoConnectorCli::Refused)
}

/// Run the connector's CLI with `args`, waiting at most `timeout`: its JSON answer when it says `ok`, else `None`.
pub fn run_connector(cli: &ConnectorCli, args: &[&str], timeout: std::time::Duration) -> Option<Value> {
    cli.run(args, timeout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_the_app_log_then_the_connectors_and_only_what_exists() {
        let dir = tempfile::tempdir().unwrap();
        let (app, core) = (dir.path().join("app.log"), dir.path().join("core.log"));
        let missing = dir.path().join("nope.log");
        assert!(logs_to_show(None, None).is_empty());
        assert!(logs_to_show(Some(missing.clone()), None).is_empty());
        std::fs::write(&app, "x").unwrap();
        assert_eq!(logs_to_show(Some(app.clone()), None), vec![app.clone()]);
        assert_eq!(logs_to_show(Some(app.clone()), Some(missing)), vec![app.clone()]);
        std::fs::write(&core, "x").unwrap();
        assert_eq!(logs_to_show(Some(app.clone()), Some(core.clone())), vec![app, core]);
    }
}
