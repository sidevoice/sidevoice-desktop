//! The local host (docs/LOCAL_HOST.md) as the room page reaches it: `window.__sidevoiceDesktop.host.localHost`.
//!
//! The work is the `sidevoice-local-host` crate's (the checks, the pairing over the core's socket, the proxy, the
//! poll); this module starts it with the app's own paths, answers the page's commands, and stops it as the app quits.
//! Offered on macOS only (design O2; `bridge::local_host_offered`): elsewhere every command refuses `unsupported`.
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
    use sidevoice_local_host::host::{computer_name, Action, Config, LocalHost};
    use sidevoice_local_host::paths::DataDirs;
    use std::sync::Arc;
    use tauri::{AppHandle, Manager};

    struct Hosted(Arc<LocalHost>);

    fn host(app: &AppHandle) -> Result<Arc<LocalHost>, Value> {
        app.try_state::<Hosted>().map(|hosted| hosted.0.clone()).ok_or_else(unsupported)
    }

    pub fn setup(app: &AppHandle) {
        if !sidevoice_desktop_core::bridge::local_host_offered() {
            return;
        }
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
}
