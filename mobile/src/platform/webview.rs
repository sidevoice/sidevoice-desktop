//! How the app's web view sits on the screen. The page owns the safe areas (the interface declares `viewport-fit=cover`
//! and pads its own edges with `env(safe-area-inset-*)`), so the web view has to fill the screen under the system bars,
//! and what is behind it where it does not reach
//! must be the system's background (light or dark with the device, as the page is), not an unset colour.
//!
//! Only iOS asks the app to do it: its window has no background colour set (nothing is drawn there but black), and
//! the web view takes the frame it was created with. Android's edge-to-edge window is the Android library's
//! (`mobile/plugin`).
use tauri::webview::PlatformWebview;

#[cfg_attr(target_os = "android", path = "webview/android.rs")]
#[cfg_attr(target_os = "ios", path = "webview/ios.rs")]
mod imp;
use imp::Native;

/// What differs per OS: how the web view is made to fill the screen and keep its scale, and what it can say of where it is.
trait Os {
    /// Makes the web view fill the screen, locks its zoom (an app is not a document), and gives everything behind it the
    /// system's background (light or dark with the device) instead of an unset colour. Idempotent: it runs again once the
    /// window is on screen. Runs on the main thread (`WebviewWindow::with_webview`).
    fn fill_screen(webview: &PlatformWebview);

    /// Where the window, its view and the web view are, and the safe-area insets, in points: `None` where there is no
    /// such thing to describe. Runs on the main thread.
    fn layout(webview: &PlatformWebview) -> Option<String>;
}

/// Makes the web view fill the screen, under the status bar and the home indicator.
pub fn fill_screen(webview: &PlatformWebview) {
    Native::fill_screen(webview);
}

/// Logs where the web view is (a device test shows what the screen really holds, not what the code assumes).
pub fn log_layout(when: &str, webview: &PlatformWebview) {
    if let Some(layout) = Native::layout(webview) {
        log::info!("layout {when}: {layout}");
    }
}
