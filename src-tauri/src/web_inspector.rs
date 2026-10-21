//! The Web Inspector switch of the developer options (`window.__sidevoiceDesktop.host.debug`, docs/BRIDGE.md → "The
//! developer options"): in a nightly or a dev build (core `build_info::debug_offered`), Safari's Web Inspector can be
//! attached to the room and to the call controls card. Off by default; the choice is kept in the settings and applied
//! to both webviews as they are created and when the page changes it, with no reload.
//!
//! macOS: `WKWebView.isInspectable` (macOS 13.3+; before that a webview cannot be attached to, and the switch stays
//! off). Windows and Linux: nothing is wired (no WebView2 or WebKitGTK binding is a dependency), so the switch does
//! nothing, is not kept, and `inspectable` stays `false`.

use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Webview, WebviewWindow};

use crate::{refusal, AppState};

/// Whether this build has the developer options (build.rs says the channel).
fn offered() -> bool {
    sidevoice_desktop_core::build_info::debug_offered(env!("SIDEVOICE_BUILD_CHANNEL"))
}

/// `host.debug`'s one command: `action` `set` with `enabled`, answering `{inspectable}`: what took, which is `false`
/// where the system cannot. Refuses `bad_request` outside a nightly or dev build, for another action or caller.
#[tauri::command]
pub async fn web_inspector(
    app: AppHandle,
    webview: Webview,
    action: String,
    enabled: Option<bool>,
) -> Result<Value, Value> {
    crate::room_page(&app, &webview).map_err(|why| refusal("bad_request", why))?;
    if !offered() {
        return Err(refusal("bad_request", "The developer options are not in this build."));
    }
    match (action.as_str(), enabled) {
        ("set", Some(enabled)) => {
            let on = enabled && supported();
            crate::change_settings(&app, |s| s.web_inspector = on).map_err(|e| refusal("internal", e))?;
            apply(&app);
            Ok(json!({ "inspectable": on }))
        }
        _ => Err(refusal("bad_request", format!("not a web inspector action: {action}"))),
    }
}

/// Puts the kept choice on the webviews that exist: the room and the card. Called as each is created, and on a change.
pub(crate) fn apply(app: &AppHandle) {
    if !offered() {
        return;
    }
    let on = app
        .state::<AppState>()
        .settings
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|s| s.web_inspector);
    for window in [crate::room_window(app), app.get_webview_window(crate::call_controls::LABEL)].into_iter().flatten() {
        set(&window, on);
    }
}

#[cfg(target_os = "macos")]
fn supported() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
fn supported() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn set(window: &WebviewWindow, on: bool) {
    use objc2::{msg_send, runtime::AnyObject, runtime::Bool, sel};
    let _ = window.with_webview(move |webview| {
        let view = webview.inner() as *mut AnyObject;
        if view.is_null() {
            return;
        }
        // SAFETY: the window's own WKWebView, on the main thread (`with_webview`); `setInspectable:` only where the
        // system has it (macOS 13.3+).
        unsafe {
            let responds: bool = msg_send![view, respondsToSelector: sel!(setInspectable:)];
            if responds {
                let _: () = msg_send![view, setInspectable: Bool::new(on)];
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn set(_: &WebviewWindow, _: bool) {}
