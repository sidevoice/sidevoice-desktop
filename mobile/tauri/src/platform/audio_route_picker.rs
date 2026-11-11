//! The system's own picker for where the call's audio plays (`host.audioRoute`, docs/BRIDGE.md -> "The audio route"). An
//! app cannot choose an arbitrary output on iOS (the speaker, the car, AirPods, AirPlay): only the system's sheet can, and
//! the app opens it from a button of its own. Android has no such sheet to open.
use tauri::webview::PlatformWebview;

#[cfg_attr(target_os = "android", path = "audio_route_picker/android.rs")]
#[cfg_attr(target_os = "ios", path = "audio_route_picker/ios.rs")]
mod imp;
use imp::Native;

/// What differs per OS.
trait Os {
    /// Whether this phone has a system output picker the app can open.
    const AVAILABLE: bool;
    /// Opens it over the app's window. Runs on the main thread (`Webview::with_webview`).
    fn open(webview: &PlatformWebview) -> Result<(), String>;
}

/// Whether the page may offer the picker's button.
pub fn available() -> bool {
    Native::AVAILABLE
}

/// Opens the system's picker. A stable code on refusal.
pub fn open(webview: &PlatformWebview) -> Result<(), String> {
    Native::open(webview)
}
