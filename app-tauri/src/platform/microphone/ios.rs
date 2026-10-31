//! The iOS body: AVFoundation's permission (`apple.rs`; the prompt shows the app's `NSMicrophoneUsageDescription`) and the
//! app's own page of Settings, where the microphone switch is.
use super::{apple, AccessState, Os};

pub struct Native;
impl Os for Native {
    /// `UIApplication.openSettingsURLString`.
    const SETTINGS_URL: Option<&'static str> = Some("app-settings:");

    fn permission() -> Result<AccessState, &'static str> {
        apple::permission()
    }

    async fn ask() -> Result<AccessState, &'static str> {
        apple::ask().await
    }
}
