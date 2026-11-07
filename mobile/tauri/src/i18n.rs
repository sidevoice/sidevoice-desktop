//! The mobile app's own texts, the ones the page shows but the app words (docs/MOBILE.md -> "The bridge"): how to allow a
//! blocked microphone on this phone, and the button that opens that settings page (D-HOST-OS: each app words its own
//! system's help). One JSON table per language (`i18n/native/<language>.json`, the eleven the web offers), English the
//! fallback; adding a text means adding its key to English. The loader is sidevoice-shared-i18n's.
use sidevoice_shared_i18n::{Bundles, Texts};

pub use sidevoice_shared_i18n::LANGUAGES;

/// The app's tables, one per language of [`LANGUAGES`].
const BUNDLES: Bundles = [
    ("en", include_str!("../i18n/native/en.json")),
    ("de", include_str!("../i18n/native/de.json")),
    ("es", include_str!("../i18n/native/es.json")),
    ("fr", include_str!("../i18n/native/fr.json")),
    ("it", include_str!("../i18n/native/it.json")),
    ("ja", include_str!("../i18n/native/ja.json")),
    ("pl", include_str!("../i18n/native/pl.json")),
    ("zh-Hans", include_str!("../i18n/native/zh-Hans.json")),
    ("pt-BR", include_str!("../i18n/native/pt-BR.json")),
    ("ko", include_str!("../i18n/native/ko.json")),
    ("hi", include_str!("../i18n/native/hi.json")),
];

static TEXTS: Texts = Texts::new(&BUNDLES);

/// `key` in `language`, else in English; the key itself when English lacks it too (a test keeps that from happening).
pub fn t(language: &str, key: &'static str) -> &'static str {
    TEXTS.get(language, key).unwrap_or(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_has_every_english_key_and_no_other_each_a_text() {
        assert_eq!(sidevoice_shared_i18n::check(&BUNDLES), Ok(()));
    }

    #[test]
    fn the_help_is_worded_in_each_language() {
        for key in [
            "microphone.help_ios",
            "microphone.help_android",
            "microphone.open_settings_ios",
            "microphone.open_settings_android",
        ] {
            for language in LANGUAGES.iter().filter(|language| **language != "en") {
                assert_ne!(t(language, key), t("en", key), "{language} {key}");
            }
        }
    }

    #[test]
    fn an_unknown_language_is_english() {
        assert_eq!(t("ru", "microphone.open_settings_ios"), "Open Settings");
        assert_eq!(t("es", "microphone.open_settings_ios"), "Abrir Ajustes");
        assert_eq!(
            t("de", "microphone.open_settings_ios"),
            "Einstellungen öffnen"
        );
    }
}
