//! iOS: the app's own page of Settings, where its Microphone switch is.
use super::Os;

pub struct Native;

impl Os for Native {
    const HELP: Option<&'static str> = Some("microphone.help_ios");
    const SETTINGS_LABEL: Option<&'static str> = Some("microphone.open_settings_ios");
}
