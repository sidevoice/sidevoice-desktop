//! The developer options (`window.__sidevoiceDesktop.host.debug`, docs/BRIDGE.md → "The developer options"): in a
//! nightly or a dev build, switches the room's hidden "Developer options" section turns on the room's and the call
//! controls card's webviews. Off by default; kept in the settings (core `debug_options`) and put on both webviews as
//! they are created and when the page changes one, with no reload.
//!
//! macOS: `WKWebView.isInspectable` (Safari's Web Inspector, macOS 13.3+), and WebKit's private debug preferences
//! `_setCompositingBordersVisible:` (each composited layer outlined: what the GPU draws) and
//! `_setCompositingRepaintCountersVisible:` (each layer's repaint count: what the CPU keeps painting), each asked only of
//! what answers to it. Windows and Linux: none (core `debug_options::available`).
//!
//! The native part is compiled only into a nightly or a dev build (`sidevoice_debug_options`, set by build.rs); in a
//! release the command refuses and nothing touches WebKit's private API.

use serde_json::Value;
use tauri::{AppHandle, Webview};

#[cfg(sidevoice_debug_options)]
use sidevoice_desktop_core::debug_options::{available, DebugOptions, Switch};
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
    let switch = name.and_then(Switch::parse).filter(|s| available().contains(s));
    match (action, switch, enabled) {
        ("set", Some(switch), Some(on)) => {
            let kept = crate::change_settings(app, |s| s.debug.set(switch, on)).map_err(|e| refusal("internal", e))?;
            apply(app);
            Ok(kept.debug.state(available()))
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
    let options = app.state::<AppState>().settings.lock().unwrap().as_ref().map(|s| s.debug).unwrap_or_default();
    for window in [crate::room_window(app), app.get_webview_window(crate::call_controls::LABEL)].into_iter().flatten() {
        put(&window, options);
    }
}

#[cfg(not(sidevoice_debug_options))]
pub(crate) fn apply(_: &AppHandle) {}

#[cfg(all(sidevoice_debug_options, target_os = "macos"))]
fn put(window: &tauri::WebviewWindow, options: DebugOptions) {
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{msg_send, sel};
    let _ = window.with_webview(move |webview| {
        let view = webview.inner() as *mut AnyObject;
        if view.is_null() {
            return;
        }
        // SAFETY: the window's own WKWebView and its live preferences, on the main thread (`with_webview`); each setter
        // only where the system answers to it.
        unsafe {
            let responds: bool = msg_send![view, respondsToSelector: sel!(setInspectable:)];
            if responds {
                let _: () = msg_send![view, setInspectable: Bool::new(options.inspectable)];
            }
            let configuration: *mut AnyObject = msg_send![view, configuration];
            if configuration.is_null() {
                return;
            }
            let preferences: *mut AnyObject = msg_send![configuration, preferences];
            if preferences.is_null() {
                return;
            }
            let responds: bool = msg_send![preferences, respondsToSelector: sel!(_setCompositingBordersVisible:)];
            if responds {
                let _: () =
                    msg_send![preferences, _setCompositingBordersVisible: Bool::new(options.compositing_borders)];
            }
            let responds: bool =
                msg_send![preferences, respondsToSelector: sel!(_setCompositingRepaintCountersVisible:)];
            if responds {
                let _: () =
                    msg_send![preferences, _setCompositingRepaintCountersVisible: Bool::new(options.repaint_counters)];
            }
        }
    });
}

#[cfg(all(sidevoice_debug_options, not(target_os = "macos")))]
fn put(_: &tauri::WebviewWindow, _: DebugOptions) {}
