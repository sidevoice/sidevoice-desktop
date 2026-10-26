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
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::NativeCardWindow;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::NativeCardWindow;

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
        card_builder(app, spec).build()?;
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

/// Shared Tauri construction options; Windows also adjusts the resulting native handle.
fn card_builder(app: &AppHandle, spec: CardSpec) -> tauri::WebviewWindowBuilder<'_, tauri::Wry, AppHandle> {
    let builder = tauri::WebviewWindowBuilder::new(app, spec.label, WebviewUrl::App(spec.page.into()))
        .inner_size(spec.size.width, spec.size.height)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .focusable(false)
        .shadow(false);
    window_options(builder, spec.script)
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

// This regression needs real Win32 messages: a mock Tauri window cannot exercise WM_STYLECHANGING.
#[cfg(all(test, target_os = "windows"))]
mod tests {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_APPWINDOW,
        WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
    };

    #[test]
    fn switcher_exclusion_survives_native_style_rewrites() {
        // SAFETY: STATIC is a predefined system class. The hidden test window is owned, mutated and
        // destroyed on this test thread; no event loop or application webview is required.
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_APPWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
                windows_sys::core::w!("STATIC"),
                windows_sys::core::w!("card style regression"),
                WS_POPUP,
                0,
                0,
                10,
                10,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!hwnd.is_null());
            struct Window(windows_sys::Win32::Foundation::HWND);
            impl Drop for Window {
                fn drop(&mut self) {
                    unsafe {
                        DestroyWindow(self.0);
                    }
                }
            }
            let window = Window(hwnd);
            super::windows::exclude_from_switcher(window.0).unwrap();
            let policy = (WS_EX_APPWINDOW | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED) as isize;
            let expected = (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED) as isize;
            assert_eq!(GetWindowLongPtrW(window.0, GWL_EXSTYLE) & policy, expected);
            // Simulate Tao regenerating the extended styles when visibility changes. The subclass
            // must preserve exclusion and nonactivation while leaving unrelated style bits alone.
            SetWindowLongPtrW(window.0, GWL_EXSTYLE, (WS_EX_APPWINDOW | WS_EX_LAYERED) as isize);
            assert_eq!(GetWindowLongPtrW(window.0, GWL_EXSTYLE) & policy, expected);
        }
    }
}
