//! Android: the app's page of the system's settings, where its permissions are.
use super::Os;

pub struct Native;

impl Os for Native {
    const HELP: Option<&'static str> = Some("microphone.help_android");
    const SETTINGS_LABEL: Option<&'static str> = Some("microphone.open_settings_android");
}
