//! The local host (docs/LOCAL_HOST.md) as the room page reaches it: `window.__sidevoiceDesktop.host.localHost`.
//!
//! The work is the `sidevoice-local-host` crate's (the checks, the pairing over the core's socket, the proxy, the
//! poll); this module starts it with the app's own paths, answers the page's commands, and stops it as the app quits.
//! Offered on macOS only (design O2; `platform::host`): elsewhere every command refuses `unsupported`,
//! except `reveal-log`, which needs no local host (the app's own log).
//!
//! Only the room window's own page may call these (capabilities/room.json, and [`crate::room_page`] in each). A
//! refusal is `{key, message}`, as the native engine's (docs/BRIDGE.md).

use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Webview};

/// A command's answer: a value, or a refusal `{key, message}`.
type Answer = Result<Value, Value>;

fn refusal(key: &str, message: &str) -> Value {
    json!({ "key": key, "message": message })
}

fn unsupported() -> Value {
    refusal("unsupported", "This app offers no local host on this system.")
}

fn caller(app: &AppHandle, webview: &Webview) -> Result<(), Value> {
    crate::room_page(app, webview).map_err(|why| {
        crate::debug(&format!("local_host refused: {why}"));
        refusal("not_room_page", &why)
    })
}

/// `{state, failure?, core?, product_version, service?, calls?, attempts?, limit?, reachable}`.
#[tauri::command]
pub fn local_host_state(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    imp::state(&app)
}

/// The pairing the page uses for the local host (design §4.1), or `null` unless it is running.
#[tauri::command]
pub fn local_host_pairing(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    imp::pairing(&app)
}

/// `start`, `stop`, `restart`, `service-install`, `service-uninstall`, `start-at-login-on`, `start-at-login-off` (the
/// connector's CLI), `reconnect`, `reveal-log`: the state after it.
#[tauri::command]
pub async fn local_host_action(app: AppHandle, webview: Webview, action: String) -> Answer {
    caller(&app, &webview)?;
    let _permit = crate::data::admit(&app).ok_or_else(|| refusal("reset.busy", "Data reset is under way."))?;
    if app.state::<crate::AppState>().resetting.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(refusal("reset.busy", "Data reset is under way."));
    }
    tauri::async_runtime::spawn_blocking(move || match action.as_str() {
        "reveal-log" => reveal(&app),
        _ => imp::action(&app, &action),
    })
    .await
    .map_err(|e| refusal("internal", &e.to_string()))?
}

/// `npx sidevoice pair-device --json`, on the person's click: `{code, expires_in, reach}`.
#[tauri::command]
pub async fn local_host_pairing_code(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    let _permit = crate::data::admit(&app).ok_or_else(|| refusal("reset.busy", "Data reset is under way."))?;
    tauri::async_runtime::spawn_blocking(move || imp::pairing_code(&app))
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
    // Begun here, before the blocking work: its progress and its cancel find it from the start.
    let begun = imp::begin_install(&app, &job)?;
    tauri::async_runtime::spawn_blocking(move || imp::run_install(begun))
        .await
        .map_err(|e| refusal("internal", &e.to_string()))?
}

/// `{step, done, total, cancellable}` of install `job` while it runs; `null` for another id or once it has ended.
#[tauri::command]
pub fn local_host_install_progress(app: AppHandle, webview: Webview, job: String) -> Answer {
    caller(&app, &webview)?;
    imp::install_progress(&app, &job)
}

/// Asks install `job` to stop: true while that is honoured (download, verification, unpacking), false once the
/// package's installer runs, or for an id not under way.
#[tauri::command]
pub fn local_host_install_cancel(app: AppHandle, webview: Webview, job: String) -> Answer {
    caller(&app, &webview)?;
    imp::cancel_install(&app, &job)
}

/// Select the existing app log, and the core's (else connector) log when there is a local host, in the native file
/// manager, without a shell. The app log needs no connector, so this runs on every OS: the answer is the local host's
/// state after it, `null` where there is none.
fn reveal(app: &AppHandle) -> Answer {
    use sidevoice_desktop_core::i18n::t;

    let logs = logs_to_show(crate::app_log_file(app), imp::host_log(app));
    let language = app.state::<crate::AppState>().language;
    if logs.is_empty() {
        return Err(refusal("log.missing", t(language, "local_host.log_missing")));
    }
    crate::platform::reveal::files(app, &logs).map_err(|error| {
        log::warn!("Could not reveal logs: {error}");
        refusal("reveal.failed", t(language, "local_host.reveal_failed"))
    })?;
    Ok(imp::state(app).unwrap_or(Value::Null))
}

/// What "Ver registro" shows: the app's own log first, then the core's (else the connector's) one, each only if it is
/// a file. The app's is there with or without a connector.
fn logs_to_show(app: Option<PathBuf>, connector: Option<PathBuf>) -> Vec<PathBuf> {
    let is_file = |path: &PathBuf| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file());
    app.into_iter().chain(connector).filter(is_file).collect()
}

/// At start: the proxy and the poll, for the app's lifetime (macOS).
pub fn setup(app: &AppHandle) {
    imp::setup(app)
}

/// As the app quits: the poll and the proxy stop, every tunnel closes.
pub fn shutdown(app: &AppHandle) {
    imp::shutdown(app)
}

/// A missing install record is safe to treat as absent only when the observer also sees no service.
#[cfg(target_os = "macos")]
pub fn absent(app: &AppHandle) -> bool {
    imp::absent(app)
}

/// A connector install under way: the reset refuses before its confirmation. Never where there is no local host.
pub fn installing(app: &AppHandle) -> bool {
    imp::installing(app)
}

/// The connector's CLI as the reset runs it: the one the install in `D` recorded, checked as the local-host crate
/// checks it (`Cli::installed`). Nothing else runs the connector's `uninstall`.
pub use imp::ConnectorCli;

/// Why the reset has no connector CLI to run.
pub enum NoConnectorCli {
    /// The app cannot run the connector's CLI on this host (the local-host crate is not built here).
    Unsupported,
    /// The install record in `D` is missing or unsafe, or names nothing runnable.
    Refused,
}

/// The connector's CLI the install in `data` (`D`) recorded, for the reset's connector step.
pub fn connector_cli(data: &std::path::Path) -> Result<ConnectorCli, NoConnectorCli> {
    imp::connector_cli(data).ok_or(if imp::RUNS_CONNECTOR_CLI {
        NoConnectorCli::Refused
    } else {
        NoConnectorCli::Unsupported
    })
}

/// Run the connector's CLI with `args`, waiting at most `timeout`: its JSON answer when it says `ok`, else `None`.
pub fn run_connector(cli: &ConnectorCli, args: &[&str], timeout: std::time::Duration) -> Option<Value> {
    imp::run_connector(cli, args, timeout)
}

#[cfg(unix)]
mod imp {
    use super::{refusal, unsupported, Answer};
    use crate::platform::host;
    use serde_json::{json, Value};
    use sidevoice_local_host::host::{Action, Config, Installing, LocalHost};
    use sidevoice_local_host::install;
    use sidevoice_local_host::npm::Https;
    use sidevoice_local_host::paths::DataDirs;
    use sidevoice_local_host::platform::computer_name::{ComputerName, SystemComputerName};
    use std::sync::Arc;
    use tauri::{AppHandle, Manager};

    struct Hosted(Arc<LocalHost>);

    fn host(app: &AppHandle) -> Result<Arc<LocalHost>, Value> {
        app.try_state::<Hosted>().map(|hosted| hosted.0.clone()).ok_or_else(unsupported)
    }

    pub fn setup(app: &AppHandle) {
        if !host::capabilities().local_host {
            return;
        }
        // `D`, by the rule every Sidevoice program shares (`SIDEVOICE_DATA_DIR`, else the OS standard place).
        let Some(dirs) = DataDirs::from_env() else {
            crate::debug("local-host off: no data directory");
            return;
        };
        let Ok(app_dir) = crate::data::desktop_dir(app) else { return };
        let config = Config { dirs, app_dir, name: SystemComputerName::read() };
        match LocalHost::start(config, crate::debug) {
            Ok(host) => {
                host.watch();
                app.manage(Hosted(host));
            }
            Err(e) => crate::debug(&format!("local-host off: the proxy could not start: {e}")),
        }
    }

    pub fn shutdown(app: &AppHandle) {
        if let Some(hosted) = app.try_state::<Hosted>() {
            hosted.0.shutdown();
        }
    }

    pub fn state(app: &AppHandle) -> Answer {
        Ok(json!(host(app)?.state()))
    }

    #[cfg(target_os = "macos")]
    pub fn absent(app: &AppHandle) -> bool {
        host(app).is_ok_and(|host| json!(host.state())["state"] == "absent")
    }
    pub fn installing(app: &AppHandle) -> bool {
        host(app).is_ok_and(|host| json!(host.state())["state"] == "installing")
    }

    pub use sidevoice_local_host::cli::Cli as ConnectorCli;

    /// The local-host crate runs the connector's CLI here.
    pub const RUNS_CONNECTOR_CLI: bool = true;

    pub fn connector_cli(data: &std::path::Path) -> Option<ConnectorCli> {
        ConnectorCli::installed(&DataDirs::new(data)).ok()
    }

    pub fn run_connector(cli: &ConnectorCli, args: &[&str], timeout: std::time::Duration) -> Option<Value> {
        cli.run(args, timeout).ok()
    }

    pub fn pairing(app: &AppHandle) -> Answer {
        Ok(host(app)?.pairing().unwrap_or(Value::Null))
    }

    pub fn action(app: &AppHandle, name: &str) -> Answer {
        let host = host(app)?;
        let report = match name {
            "reconnect" => host.reconnect(),
            other => {
                let action = Action::parse(other).ok_or_else(|| refusal("bad_request", "Unknown action."))?;
                host.act(action).map_err(|r| json!(r))?
            }
        };
        Ok(json!(report))
    }

    /// The core's log (else the connector's), when there is a host to say.
    pub fn host_log(app: &AppHandle) -> Option<std::path::PathBuf> {
        host(app).ok().and_then(|host| host.log_path())
    }

    pub fn pairing_code(app: &AppHandle) -> Answer {
        host(app)?.pairing_code().map_err(|r| json!(r))
    }

    /// An install begun (the slot taken); dropped unrun, it frees it.
    pub struct Begun(Installing);

    pub fn begin_install(app: &AppHandle, job: &str) -> Result<Begun, Value> {
        host(app)?.begin_install(job).map(Begun).map_err(|failure| json!(failure))
    }

    /// The whole install, blocking (on the async runtime's blocking pool: the HTTPS client runs a runtime of its own,
    /// made and dropped here). The host logs how it went.
    pub fn run_install(begun: Begun) -> Answer {
        let Begun(installing) = begun;
        let fetch = Https::new().map_err(|failure| json!(failure))?;
        installing
            .run(&fetch, &install::Options::standard())
            .map(|report| json!(report))
            .map_err(|failure| json!(failure))
    }

    pub fn install_progress(app: &AppHandle, job: &str) -> Answer {
        Ok(host(app)?.install_progress(job).map(|progress| json!(progress)).unwrap_or(Value::Null))
    }

    pub fn cancel_install(app: &AppHandle, job: &str) -> Answer {
        Ok(json!(host(app)?.cancel_install(job)))
    }
}

#[cfg(not(unix))]
mod imp {
    use super::{unsupported, Answer};
    use tauri::AppHandle;

    pub fn setup(_app: &AppHandle) {}
    pub fn shutdown(_app: &AppHandle) {}
    pub fn state(_app: &AppHandle) -> Answer {
        Err(unsupported())
    }
    pub fn pairing(_app: &AppHandle) -> Answer {
        Err(unsupported())
    }
    pub fn action(_app: &AppHandle, _name: &str) -> Answer {
        Err(unsupported())
    }
    pub fn pairing_code(_app: &AppHandle) -> Answer {
        Err(unsupported())
    }
    pub fn host_log(_app: &AppHandle) -> Option<std::path::PathBuf> {
        None
    }
    pub fn installing(_app: &AppHandle) -> bool {
        false
    }
    /// No connector CLI the app can run here: never made.
    pub enum ConnectorCli {}
    pub const RUNS_CONNECTOR_CLI: bool = false;
    pub fn connector_cli(_data: &std::path::Path) -> Option<ConnectorCli> {
        None
    }
    pub fn run_connector(
        cli: &ConnectorCli,
        _args: &[&str],
        _timeout: std::time::Duration,
    ) -> Option<serde_json::Value> {
        match *cli {}
    }
    pub struct Begun;
    pub fn begin_install(_app: &AppHandle, _job: &str) -> Result<Begun, serde_json::Value> {
        Err(unsupported())
    }
    pub fn run_install(_begun: Begun) -> Answer {
        Err(unsupported())
    }
    pub fn install_progress(_app: &AppHandle, _job: &str) -> Answer {
        Err(unsupported())
    }
    pub fn cancel_install(_app: &AppHandle, _job: &str) -> Answer {
        Err(unsupported())
    }
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
