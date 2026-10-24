//! Microphone privacy settings only. Permission reads/requests remain with the existing macOS implementation
//! or the webview; the wider microphone access interface is separate work.
//!
//! Only the URI differs by OS, so one plugin-backed implementation serves every host.

use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

#[cfg(target_os = "macos")]
pub const SETTINGS_URL: Option<&str> =
    Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone");
#[cfg(target_os = "windows")]
pub const SETTINGS_URL: Option<&str> = Some("ms-settings:privacy-microphone");
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const SETTINGS_URL: Option<&str> = None;

/// Opens the system's microphone privacy page. No permission is requested or inferred from opening it.
pub fn open_settings(app: &AppHandle) -> Result<(), tauri_plugin_opener::Error> {
    let url = SETTINGS_URL.ok_or(tauri_plugin_opener::Error::UnsupportedPlatform)?;
    app.opener().open_url(url, None::<&str>)
}
