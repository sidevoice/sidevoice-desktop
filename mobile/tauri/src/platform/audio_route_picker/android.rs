//! Android: no system output picker for the app to open (the volume panel and the quick settings are the system's).
use tauri::webview::PlatformWebview;

use super::{Changed, Os, Presentation};

pub struct Native;

impl Os for Native {
    const AVAILABLE: bool = false;

    fn presentation(_webview: &PlatformWebview) -> Presentation {
        Presentation::default()
    }

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
