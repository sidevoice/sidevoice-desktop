//! The app's own wording of the web's texts that a phone changes, per OS (`host.messages`; `i18n/<os>/<language>.json`, over
//! what every app shares in sidevoice-shared-tauri's `i18n/`). The speaker is chosen from Control Center on an iPhone and from
//! the quick settings on Android, so each OS has its own table (D-HOST-OS: the page shows what the app hands it).
#[cfg_attr(target_os = "android", path = "message_overlay/android.rs")]
#[cfg_attr(target_os = "ios", path = "message_overlay/ios.rs")]
mod imp;
use imp::Native;

/// What differs per OS: its tables, one JSON object of web catalogue keys to text for each language of
/// `sidevoice_shared_tauri::messages::LANGUAGES`, in that order.
trait Os {
    const TEXTS: [(&'static str, &'static str); 8];
}

/// This OS's own tables, as `(language, json)`.
pub fn texts() -> &'static [(&'static str, &'static str)] {
    &Native::TEXTS
}
