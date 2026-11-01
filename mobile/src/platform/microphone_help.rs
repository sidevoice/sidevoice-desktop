//! Which of the app's texts say how to allow a blocked microphone on this OS, and label the button that opens its
//! settings page (`crate::i18n`). Each phone has its own words; the page shows what the app hands it (D-HOST-OS).
#[cfg_attr(target_os = "android", path = "microphone_help/android.rs")]
#[cfg_attr(target_os = "ios", path = "microphone_help/ios.rs")]
mod imp;
use imp::Native;

/// What differs per OS: the keys of its texts, where it has them.
trait Os {
    /// The steps to allow the microphone.
    const HELP: Option<&'static str>;
    /// The label of the button that opens the settings page.
    const SETTINGS_LABEL: Option<&'static str>;
}

/// The key of the steps to allow a blocked microphone here, if the app words them on this OS.
pub fn help() -> Option<&'static str> {
    Native::HELP
}

/// The key of the settings button's label, if the app words it on this OS.
pub fn settings_label() -> Option<&'static str> {
    Native::SETTINGS_LABEL
}
