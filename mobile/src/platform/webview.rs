//! How the app's web view sits on the screen. The page owns the safe areas (the interface declares `viewport-fit=cover`
//! and pads its own edges with `env(safe-area-inset-*)`), so the web view has to run edge to edge and report the real
//! insets to the page, rather than have the OS shrink the page's viewport and leave the strip it vacates unpainted.
//!
//! Only iOS asks the app to do it: its web view insets its own content by the safe area unless told otherwise. Android's
//! edge-to-edge window is the Android library's (`mobile/plugin`), and a computer has no safe area.
use tauri::webview::PlatformWebview;

#[cfg_attr(target_os = "ios", path = "webview/ios.rs")]
#[cfg_attr(not(target_os = "ios"), path = "webview/other.rs")]
mod imp;
use imp::Native;

/// What differs per OS: whether the app has to tell the web view to leave the safe areas to the page.
trait Os {
    /// Makes the web view's content run under the system bars, leaving their insets to the page. Runs on the main thread
    /// (`WebviewWindow::with_webview`).
    fn cover_screen(webview: &PlatformWebview);
}

/// Lets the page's content run edge to edge under the status bar and the home indicator.
pub fn cover_screen(webview: &PlatformWebview) {
    Native::cover_screen(webview);
}
