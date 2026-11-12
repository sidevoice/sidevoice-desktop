//! Android: no system output picker for the app to open (the volume panel and the quick settings are the system's).
use tauri::webview::PlatformWebview;

use super::{Changed, Os};

pub struct Native;

impl Os for Native {
    const AVAILABLE: bool = false;

    fn open(_webview: &PlatformWebview) -> Result<(), String> {
        Err("audio-route-picker-unavailable".into())
    }

    fn current() -> Option<String> {
        None
    }

    fn observe(_changed: Changed) -> Result<(), String> {
        Ok(())
    }
}
