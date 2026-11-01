//! How the app's web view sits on the screen. The page owns the safe areas (the interface declares `viewport-fit=cover`
//! and pads its own edges with `env(safe-area-inset-*)`), so the web view has to fill the screen under the system bars,
//! and nothing behind it may show a colour of its own where it does not reach.
//!
//! Only iOS asks the app to do it: its window has no background of its own (black wherever nothing is drawn), and
//! the web view takes the frame it was created with. Android's edge-to-edge window is the Android library's
//! (`mobile/plugin`), and a computer has no safe area.
use tauri::webview::PlatformWebview;

#[cfg_attr(target_os = "ios", path = "webview/ios.rs")]
#[cfg_attr(not(target_os = "ios"), path = "webview/other.rs")]
mod imp;
use imp::Native;

/// What differs per OS: how the web view is made to fill the screen, and what it can say of where it is.
trait Os {
    /// Makes the web view fill the screen and gives everything behind it the system's background, so a part the page does
    /// not cover is never black. Runs on the main thread (`WebviewWindow::with_webview`).
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
