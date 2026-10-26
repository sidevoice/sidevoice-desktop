//! The developer options (`window.__sidevoiceDesktop.host.debug`, docs/BRIDGE.md → "The developer options"): in a
//! nightly or a dev build, switches the room's hidden "Developer options" section turns on the room's and the call
//! controls card's webviews. Off by default; kept in the settings (core `debug_options`) and put on both webviews as
//! they are created and when the page changes one, with no reload.
//!
//! macOS: `WKWebView.isInspectable` (Safari's Web Inspector, macOS 13.3+), and WebKit's private debug preferences
//! `_setCompositingBordersVisible:` (each composited layer outlined: what the GPU draws) and
//! `_setCompositingRepaintCountersVisible:` (each layer's repaint count: what the CPU keeps painting), each asked only of
//! what answers to it. Windows and Linux: none (platform `webview_diagnostics`).
//!
//! The native part is compiled only into a nightly or a dev build (`sidevoice_debug_options`, set by build.rs); in a
//! release the command refuses and nothing touches WebKit's private API.

use serde_json::Value;
use tauri::{AppHandle, Webview};

#[cfg(sidevoice_debug_options)]
use crate::platform::webview_diagnostics::{NativeWebviewDiagnostics, WebviewDiagnostics};
#[cfg(sidevoice_debug_options)]
use sidevoice_desktop_core::debug_options::Switch;
#[cfg(sidevoice_debug_options)]
use tauri::Manager;

use crate::refusal;
#[cfg(sidevoice_debug_options)]
use crate::AppState;

/// `host.debug`'s one command: `action` `set` with a switch's `name` and `enabled`, answering the switches' state
/// (core `DebugOptions::state`). Refuses `bad_request` in a release, for a switch this platform lacks, another action or
/// another caller.
#[tauri::command]
pub async fn debug_options(
    app: AppHandle,
    webview: Webview,
    action: String,
    name: Option<String>,
    enabled: Option<bool>,
) -> Result<Value, Value> {
    crate::room_page(&app, &webview).map_err(|why| refusal("bad_request", why))?;
    set(&app, &action, name.as_deref(), enabled)
}

#[cfg(sidevoice_debug_options)]
fn set(app: &AppHandle, action: &str, name: Option<&str>, enabled: Option<bool>) -> Result<Value, Value> {
    let switch = name.and_then(Switch::parse).filter(|s| NativeWebviewDiagnostics::available().contains(s));
    match (action, switch, enabled) {
        ("set", Some(switch), Some(on)) => {
            let kept = crate::change_settings(app, |s| s.debug.set(switch, on)).map_err(|e| refusal("internal", e))?;
            apply(app);
            Ok(kept.debug.state(&NativeWebviewDiagnostics::available()))
        }
        _ => Err(refusal("bad_request", format!("not a developer option: {action} {name:?}"))),
    }
}

#[cfg(not(sidevoice_debug_options))]
fn set(_: &AppHandle, _: &str, _: Option<&str>, _: Option<bool>) -> Result<Value, Value> {
    Err(refusal("bad_request", "The developer options are not in this build."))
}

/// Puts the kept switches on the webviews that exist: the room and the card. Called as each is created, and on a change.
#[cfg(sidevoice_debug_options)]
pub(crate) fn apply(app: &AppHandle) {
    let available = NativeWebviewDiagnostics::available();
    if available.is_empty() {
        return;
    }
    let mut options = app.state::<AppState>().settings.lock().unwrap().as_ref().map(|s| s.debug).unwrap_or_default();
    for switch in Switch::ALL {
        if !available.contains(&switch) {
            options.set(switch, false);
        }
    }
    for window in [crate::room_window(app), app.get_webview_window(crate::call_controls::LABEL)].into_iter().flatten() {
        if let Err(error) = NativeWebviewDiagnostics::apply(&window, options) {
            log::warn!("webview diagnostics: {error}");
        }
    }
}

#[cfg(not(sidevoice_debug_options))]
pub(crate) fn apply(_: &AppHandle) {}
