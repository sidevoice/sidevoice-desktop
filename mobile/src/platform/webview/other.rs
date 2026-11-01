//! Android and computers: nothing to do. Android's window is edge to edge from the Android library of `mobile/plugin`,
//! and a computer has no safe area.
use super::Os;
use tauri::webview::PlatformWebview;

pub struct Native;

impl Os for Native {
    fn cover_screen(_webview: &PlatformWebview) {}
}
