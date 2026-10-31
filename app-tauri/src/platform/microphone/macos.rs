//! The macOS body: AVFoundation's permission (`apple.rs`) and System Settings' microphone pane.
use super::{apple, AccessState, Os};

pub struct Native;
impl Os for Native {
    const SETTINGS_URL: Option<&'static str> =
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone");

    fn permission() -> Result<AccessState, &'static str> {
        apple::permission()
    }

    async fn ask() -> Result<AccessState, &'static str> {
        apple::ask().await
    }
}
