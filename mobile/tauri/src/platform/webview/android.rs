//! Android: nothing to do. Its window is edge to edge from the Android library of `mobile/tauri/plugin`.
use super::Os;
use tauri::webview::PlatformWebview;

pub struct Native;

impl Os for Native {
    fn fill_screen(_webview: &PlatformWebview) {}

    fn layout(_webview: &PlatformWebview) -> Option<String> {
        None
    }
}
