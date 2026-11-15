//! The crash reports command: `host.crashReports`'s `get` and `set` (docs/BRIDGE.md → "Crash reports"). The setting is the
//! app's, kept by sidevoice-shared's reporter (`crash`); the room page only reads it and passes on the person's choice.

use std::sync::Arc;

use serde_json::{json, Value};
use sidevoice_shared::crash::{Mode, Reporter};
use tauri::{Runtime, State, Webview};

use crate::Callers;

/// The app's reporter, as the plugin's state: the app manages one.
pub struct CrashReports(pub Arc<Reporter>);

/// A refusal as the page reads it (docs/BRIDGE.md → "Refusals").
fn refusal(key: &str, message: impl std::fmt::Display) -> Value {
    json!({ "key": key, "message": message.to_string() })
}

/// `get`: `{mode}`, `auto` or `never`. `install_id`: `{installId}`, the id the app's own reports carry, `null` under `never` (the
/// id is forgotten then). `set` with `mode`: keeps the choice (`never` stops sending at once, queued reports
/// included) and answers `{mode}`; refuses `internal` when the app could not keep it.
#[tauri::command]
pub async fn crash_reports<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    reports: State<'_, CrashReports>,
    action: String,
    mode: Option<String>,
) -> Result<Value, Value> {
    callers.check(&webview).map_err(|why| refusal("bad_request", why))?;
    match (action.as_str(), mode.as_deref().and_then(Mode::parse)) {
        ("get", _) => Ok(json!({ "mode": reports.0.mode().name() })),
        ("install_id", _) => Ok(json!({ "installId": reports.0.installation_id() })),
        ("set", Some(mode)) => match reports.0.set_mode(mode) {
            Ok(()) => Ok(json!({ "mode": mode.name() })),
            Err(error) => {
                tracing::warn!(
                    error.type = sidevoice_shared::error::SharedError::CrashSettingsNotKept.code(),
                    kind = ?error.kind(),
                    "the crash reports setting could not be kept"
                );
                Err(refusal("internal", "The crash reports setting could not be kept."))
            }
        },
        _ => Err(refusal("bad_request", format!("not a crash reports action: {action}"))),
    }
}
