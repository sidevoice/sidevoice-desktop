//! Non-activating call-card window construction and visibility. Native access stays here.
use crate::debug;
use sidevoice_desktop_core::call_controls::Size;
use sidevoice_desktop_core::settings::{self, APP_ORIGIN};
use tauri::utils::config::BackgroundThrottlingPolicy;
use tauri::webview::{PermissionKind, PermissionResponse};
use tauri::{AppHandle, Manager, WebviewUrl};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::NativeCardWindow;
#[cfg(not(target_os = "macos"))]
pub struct NativeCardWindow;
#[cfg(not(target_os = "macos"))]
impl CardWindow for NativeCardWindow {}

pub struct CardSpec {
    pub label: &'static str,
    pub page: &'static str,
    pub size: Size,
    pub script: String,
}

pub trait CardWindow {
    /// Install native window support before building the app.
    fn configure(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
        builder
    }
    /// Create a hidden card during setup, without taking keyboard focus. Pointer input remains enabled.
    /// Window managers may ignore stacking/workspace requests; see docs/CALL_CARD.md.
    fn create(app: &AppHandle, spec: CardSpec) -> tauri::Result<()> {
        let builder = tauri::WebviewWindowBuilder::new(app, spec.label, WebviewUrl::App(spec.page.into()))
            .inner_size(spec.size.width, spec.size.height)
            .always_on_top(true)
            .visible_on_all_workspaces(true)
            .skip_taskbar(true)
            .focusable(false)
            .shadow(false);
        window_options(builder, spec.script).build()?;
        Ok(())
    }
    /// Show or hide on the main thread, without requesting activation. No application lock may be held.
    fn set_visible(app: &AppHandle, label: &str, visible: bool) {
        if let Some(w) = app.get_webview_window(label) {
            let _ = if visible { w.show() } else { w.hide() };
        }
    }
    /// Destroy the card and its webview during shutdown.
    fn close(app: &AppHandle, label: &str) -> tauri::Result<()> {
        if let Some(w) = app.get_webview_window(label) {
            w.destroy()?;
        }
        Ok(())
    }
    /// Native activation diagnostics for the existing input regression probe; empty when unavailable.
    fn app_active() -> String {
        String::new()
    }
}

/// What every platform's card window has.
fn window_options<'a, R: tauri::Runtime, M: Manager<R>>(
    builder: tauri::WebviewWindowBuilder<'a, R, M>,
    script: String,
) -> tauri::WebviewWindowBuilder<'a, R, M> {
    builder
        .title("Sidevoice")
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .visible(false)
        .focused(false)
        .accept_first_mouse(true)
        .initialization_script(script)
        // It shows a live call: never suspend or throttle it.
        .background_throttling(BackgroundThrottlingPolicy::Disabled)
        .on_navigation(|url| {
            sidevoice_desktop_core::media::navigation_allowed(url.scheme(), &settings::url_origin(url), APP_ORIGIN)
        })
        // The card captures nothing: the microphone is the room's. Without a handler WebKit would grant.
        .on_permission_request(|_webview, kind: PermissionKind| {
            debug(&format!("call-controls permission {kind:?} denied"));
            PermissionResponse::Deny
        })
}
