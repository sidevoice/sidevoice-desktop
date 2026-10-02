//! The local host (docs/LOCAL_HOST.md) as the room page reaches it: `window.__sidevoiceDesktop.host.localHost`.
//!
//! The work is the `sidevoice-local-host` crate's (the checks, the pairing over the core's socket, the proxy, the
//! poll); this module starts it with the app's own paths, answers the page's commands, and stops it as the app quits.
//! Offered on macOS arm64 only (design O2; `bridge::local_host_offered`): elsewhere every command refuses `unsupported`.
//!
//! Only the room window's own page may call these (capabilities/room.json, and [`crate::room_page`] in each). A
//! refusal is `{key, message}`, as the native engine's (docs/BRIDGE.md).

use serde_json::{json, Value};
use tauri::{AppHandle, Webview};

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

/// `{state, failure?, core?, service?, calls?}`.
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

/// `start`, `stop`, `restart`, `service-install`, `service-uninstall` (the connector's CLI), `reconnect`, `reveal-log`:
/// the state after it.
#[tauri::command]
pub async fn local_host_action(app: AppHandle, webview: Webview, action: String) -> Answer {
    caller(&app, &webview)?;
    tauri::async_runtime::spawn_blocking(move || imp::action(&app, &action))
        .await
        .map_err(|e| refusal("internal", &e.to_string()))?
}

/// `sidevoice pair-device --json`, on the person's click: `{code, expires_in, reach}`.
#[tauri::command]
pub async fn local_host_pairing_code(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    tauri::async_runtime::spawn_blocking(move || imp::pairing_code(&app))
        .await
        .map_err(|e| refusal("internal", &e.to_string()))?
}

/// `sidevoice pair <url> <code> --json`: `{room}`.
#[tauri::command]
pub async fn local_host_pair_room(app: AppHandle, webview: Webview, url: String, code: String) -> Answer {
    caller(&app, &webview)?;
    tauri::async_runtime::spawn_blocking(move || imp::pair_room(&app, &url, &code))
        .await
        .map_err(|e| refusal("internal", &e.to_string()))?
}

/// Explicit install request. Its promise resolves only after the compatible local core is reachable and paired.
#[tauri::command]
pub async fn local_host_install(app: AppHandle, webview: Webview, job: String) -> Answer {
    caller(&app, &webview)?;
    tauri::async_runtime::spawn_blocking(move || imp::install(&app, &job))
        .await
        .map_err(|e| refusal("internal", &e.to_string()))?
}

/// Progress frames for one app-owned install job, ordered by sequence.
#[tauri::command]
pub fn local_host_install_progress(app: AppHandle, webview: Webview, job: String, after_sequence: u64) -> Answer {
    caller(&app, &webview)?;
    imp::install_progress(&app, &job, after_sequence)
}

/// Cancels only the named job. True means the connector acknowledged cancellation before commit.
#[tauri::command]
pub async fn local_host_cancel(app: AppHandle, webview: Webview, job: String) -> Answer {
    caller(&app, &webview)?;
    tauri::async_runtime::spawn_blocking(move || imp::cancel(&app, &job))
        .await
        .map_err(|e| refusal("internal", &e.to_string()))?
}

/// Runs the eligible connector/core transaction explicitly; there is no automatic downgrade.
#[tauri::command]
pub async fn local_host_update(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    tauri::async_runtime::spawn_blocking(move || imp::update(&app))
        .await
        .map_err(|e| refusal("internal", &e.to_string()))?
}

/// Read-only bridge and connector metadata plus update eligibility.
#[tauri::command]
pub fn local_host_version(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    imp::version(&app)
}

/// The local agent registry is owned by connector R2; R4 installs the host with registration disabled.
#[tauri::command]
pub fn local_host_agents(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    imp::agents(&app)
}

/// At start: the proxy and the poll, for the app's lifetime (macOS).
pub fn setup(app: &AppHandle) {
    imp::setup(app)
}

/// As the app quits: the poll and the proxy stop, every tunnel closes.
pub fn shutdown(app: &AppHandle) {
    imp::shutdown(app)
}

#[cfg(unix)]
mod imp {
    use super::{refusal, unsupported, Answer};
    use serde_json::{json, Value};
    use sidevoice_local_host::cli::Cli;
    use sidevoice_local_host::host::{computer_name, Action, Config, LocalHost};
    use sidevoice_local_host::paths::DataDirs;
    use sidevoice_local_host::pin::{ConnectorPin, InstalledBuild};
    use sidevoice_local_host::versioning::{self, UpdateStatus};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tauri::{AppHandle, Manager};

    const CONNECTOR_PIN: &str = include_str!("../connector-pin.json");
    static JOBS: AtomicU64 = AtomicU64::new(0);

    struct Hosted {
        host: Arc<LocalHost>,
        bundled_resource: Option<std::path::PathBuf>,
    }

    fn host(app: &AppHandle) -> Result<Arc<LocalHost>, Value> {
        app.try_state::<Hosted>().map(|hosted| hosted.host.clone()).ok_or_else(unsupported)
    }

    fn bundled_resource(app: &AppHandle) -> Result<std::path::PathBuf, Value> {
        app.try_state::<Hosted>().and_then(|hosted| hosted.bundled_resource.clone()).ok_or_else(|| {
            refusal("install.executable-missing", "The app's bundled connector resource path is unavailable.")
        })
    }

    pub fn setup(app: &AppHandle) {
        if !sidevoice_desktop_core::bridge::local_host_offered() {
            return;
        }
        // Resolve from the app bundle now, once. Runtime invocations never reconstruct a source-tree path or search PATH.
        let bundled_resource = app.path().resolve("resources/sidevoice", tauri::path::BaseDirectory::Resource).ok();
        // The connector's data directory, as the connector resolves it (`SIDEVOICE_DATA_DIR`, else ~/.sidevoice).
        let Some(dirs) = DataDirs::from_env() else {
            crate::debug("local-host off: no home directory");
            return;
        };
        let Ok(app_dir) = app.path().app_config_dir() else { return };
        let config = Config { dirs, app_dir, name: computer_name() };
        match LocalHost::start(config, crate::debug) {
            Ok(host) => {
                host.watch();
                app.manage(Hosted { host, bundled_resource });
            }
            Err(e) => crate::debug(&format!("local-host off: the proxy could not start: {e}")),
        }
    }

    pub fn shutdown(app: &AppHandle) {
        if let Some(hosted) = app.try_state::<Hosted>() {
            hosted.host.shutdown();
        }
    }

    pub fn state(app: &AppHandle) -> Answer {
        Ok(json!(host(app)?.state()))
    }

    pub fn pairing(app: &AppHandle) -> Answer {
        Ok(host(app)?.pairing().unwrap_or(Value::Null))
    }

    pub fn action(app: &AppHandle, name: &str) -> Answer {
        let host = host(app)?;
        let report = match name {
            "reconnect" => host.reconnect(),
            "reveal-log" => {
                reveal(&host)?;
                host.state()
            }
            other => {
                let action = Action::parse(other).ok_or_else(|| refusal("bad_request", "Unknown action."))?;
                host.act(action).map_err(|r| json!(r))?
            }
        };
        Ok(json!(report))
    }

    /// The log in Finder, selected (`open -R`, by its absolute path; no shell).
    fn reveal(host: &LocalHost) -> Result<(), Value> {
        let path = host.log_path().ok_or_else(|| refusal("log.missing", "There is no log on this computer yet."))?;
        if !cfg!(target_os = "macos") {
            return Err(unsupported());
        }
        match std::process::Command::new("/usr/bin/open").arg("-R").arg(&path).status() {
            Ok(status) if status.success() => Ok(()),
            _ => Err(refusal("reveal.failed", "Finder could not show the log.")),
        }
    }

    pub fn pairing_code(app: &AppHandle) -> Answer {
        host(app)?.pairing_code().map_err(|r| json!(r))
    }

    pub fn pair_room(app: &AppHandle, url: &str, code: &str) -> Answer {
        host(app)?.pair_room(url, code).map_err(|r| json!(r))
    }

    fn pin() -> Result<ConnectorPin, Value> {
        ConnectorPin::from_json(CONNECTOR_PIN).map_err(|error| json!(error))
    }

    fn bundled_cli(app: &AppHandle, host: &LocalHost, pin: &ConnectorPin) -> Result<Cli, Value> {
        let resource = bundled_resource(app)?;
        Cli::bundled(&resource, host.data_dirs(), pin).map_err(|error| json!(error))
    }

    fn next_job(prefix: &str) -> String {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
        format!("{prefix}-{now:x}-{:x}", JOBS.fetch_add(1, Ordering::Relaxed))
    }

    pub fn install(app: &AppHandle, job: &str) -> Answer {
        let host = host(app)?;
        let pin = pin()?;
        let cli = bundled_cli(app, &host, &pin)?;
        host.install_bundled(cli, job.to_string(), false).map(|report| json!(report)).map_err(|error| json!(error))
    }

    pub fn install_progress(app: &AppHandle, job: &str, after_sequence: u64) -> Answer {
        host(app)?.install_progress(job, after_sequence).map(|progress| json!(progress)).map_err(|error| json!(error))
    }

    pub fn cancel(app: &AppHandle, job: &str) -> Answer {
        Ok(json!(host(app)?.cancel_install(job)))
    }

    pub fn version(app: &AppHandle) -> Answer {
        let host = host(app)?;
        let pin = pin()?;
        let report = host.state();
        let core_api = report.core.as_ref().and_then(|core| core.get("api")).and_then(Value::as_i64);
        let record = Cli::selected_release_record(host.data_dirs()).map_err(|error| json!(error))?;
        let installed = record.as_ref().map(|record| InstalledBuild::from_release_record(record, core_api, None));
        let update = versioning::update_status(Some(&pin), installed.as_ref());
        Ok(json!({
            "bridge": sidevoice_desktop_core::bridge::VERSION,
            "bundled": pin.as_public_value(),
            "installed": installed,
            "core_api": core_api,
            "update": update,
            "capabilities": {"agents": false},
        }))
    }

    pub fn agents(app: &AppHandle) -> Answer {
        let _ = host(app)?;
        Err(refusal("agents.unavailable", "This connector does not provide agent discovery yet."))
    }

    pub fn update(app: &AppHandle) -> Answer {
        let host = host(app)?;
        let pin = pin()?;
        let report = host.state();
        let core_api = report.core.as_ref().and_then(|core| core.get("api")).and_then(Value::as_i64);
        let record = Cli::selected_release_record(host.data_dirs()).map_err(|error| json!(error))?;
        let installed = record.as_ref().map(|record| InstalledBuild::from_release_record(record, core_api, None));
        match versioning::update_status(Some(&pin), installed.as_ref()) {
            UpdateStatus::Available => {
                let cli = bundled_cli(app, &host, &pin)?;
                host.install_bundled(cli, next_job("local-update"), true)
                    .map(|report| json!(report))
                    .map_err(|error| json!(error))
            }
            UpdateStatus::Current => {
                Ok(json!({"state": report.state, "reachable": report.reachable, "result": "noop"}))
            }
            UpdateStatus::NewerInstalled => Err(refusal(
                "update.newer-installed",
                "A newer connector is already installed; the app will not downgrade it.",
            )),
            UpdateStatus::Incompatible => Err(refusal(
                "install.incompatible",
                "The installed or bundled core API/link is outside the supported range.",
            )),
            UpdateStatus::Unknown => Err(refusal(
                "update.unknown",
                "The connector metadata is incomplete, so update eligibility is unknown.",
            )),
        }
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
    pub fn pair_room(_app: &AppHandle, _url: &str, _code: &str) -> Answer {
        Err(unsupported())
    }
    pub fn install(_app: &AppHandle, _job: &str) -> Answer {
        Err(unsupported())
    }
    pub fn install_progress(_app: &AppHandle, _job: &str, _after_sequence: u64) -> Answer {
        Err(unsupported())
    }
    pub fn cancel(_app: &AppHandle, _job: &str) -> Answer {
        Err(unsupported())
    }
    pub fn update(_app: &AppHandle) -> Answer {
        Err(unsupported())
    }
    pub fn version(_app: &AppHandle) -> Answer {
        Err(unsupported())
    }
    pub fn agents(_app: &AppHandle) -> Answer {
        Err(unsupported())
    }
}
