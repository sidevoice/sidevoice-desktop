//! How the app's web view sits on the screen. The page owns the safe areas (the interface declares `viewport-fit=cover`
//! and pads its own edges with `env(safe-area-inset-*)`), so the web view has to fill the screen under the system bars,
//! and what is behind it where it does not reach
//! must be the system's background (light or dark with the device, as the page is), not an unset colour.
//!
//! Only iOS asks the app to do it: its window has no background colour set (nothing is drawn there but black), and
//! the web view takes the frame it was created with. Android's edge-to-edge window is the Android library's
//! (`mobile/tauri/plugin`).
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

    /// Handles the system ending the web view's own process while the app lives on (iOS: memory pressure, a crash): the
    /// page would stay blank, so iOS logs it and reloads the web view (a few times at most in a minute), and a reload
    /// ends the call (`call::RoomCall::page_changed`). Where the system says nothing of it, the builder comes back as it
    /// went in.
    fn watch_termination(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry>;
}

/// Makes the web view fill the screen, under the status bar and the home indicator.
pub fn fill_screen(webview: &PlatformWebview) {
    Native::fill_screen(webview);
}

/// The builder, told to handle the system ending the web view's process.
pub fn watch_termination(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    Native::watch_termination(builder)
}

/// Logs where the web view is (a device test shows what the screen really holds, not what the code assumes).
pub fn log_layout(when: &str, webview: &PlatformWebview) {
    if let Some(layout) = Native::layout(webview) {
        tracing::info!(
            when,
            layout = layout.as_str(),
            "the web view layout was measured"
        );
    }
}
