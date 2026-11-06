//! The app's native texts (tray menu, call controls, the microphone's help for the page), keyed, one JSON table per
//! language (`i18n/<language>.json`, the eight the web offers), English the fallback for a missing key or a language
//! without a bundle (AGENTS.md). The loader is sidevoice-shared-i18n's; this file keeps the app's tables and its choice
//! of language. The shell reads the system's preferred languages once at start and picks with [`language`].

use sidevoice_shared_i18n::{Bundles, Texts};

pub use sidevoice_shared_i18n::{language, LANGUAGES};

/// The app's tables, one per language of [`LANGUAGES`].
const BUNDLES: Bundles = [
    ("en", include_str!("../i18n/en.json")),
    ("de", include_str!("../i18n/de.json")),
    ("es", include_str!("../i18n/es.json")),
    ("fr", include_str!("../i18n/fr.json")),
    ("it", include_str!("../i18n/it.json")),
    ("ja", include_str!("../i18n/ja.json")),
    ("pl", include_str!("../i18n/pl.json")),
    ("zh-Hans", include_str!("../i18n/zh-Hans.json")),
];

static TEXTS: Texts = Texts::new(&BUNDLES);

/// The text for `key` in `language`, falling back to English, then to the key itself.
pub fn t(language: &str, key: &str) -> &'static str {
    TEXTS.get(language, key).unwrap_or_else(|| leak(key))
}

/// An unknown key is a bug that shows the key; never a panic in a menu.
fn leak(key: &str) -> &'static str {
    Box::leak(key.to_string().into_boxed_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MICROPHONE_HELP: [&str; 5] = [
        "microphone.help_macos",
        "microphone.help_windows",
        "microphone.help_linux",
        "microphone.open_settings",
        "microphone.open_settings_windows",
    ];

    #[test]
    fn the_system_s_first_language_with_a_bundle_wins() {
        assert_eq!(language(&["es-ES", "en-US"]), "es");
        assert_eq!(language(&["de-DE", "es_419"]), "de");
        assert_eq!(language(&["ru-RU", "fr"]), "fr");
        assert_eq!(language(&["ru-RU", "ko"]), "en", "English when nothing has a bundle");
        assert_eq!(language::<&str>(&[]), "en");
    }

    #[test]
    fn chinese_picks_the_simplified_bundle_and_not_the_traditional_one() {
        assert_eq!(language(&["zh-Hans-CN"]), "zh-Hans");
        assert_eq!(language(&["zh-CN"]), "zh-Hans");
        assert_eq!(language(&["zh"]), "zh-Hans");
        assert_eq!(language(&["zh-TW", "ja-JP"]), "ja");
    }

    #[test]
    fn every_language_has_exactly_english_s_keys_as_texts() {
        assert_eq!(sidevoice_shared_i18n::check(&BUNDLES), Ok(()));
    }

    #[test]
    fn the_microphone_help_is_worded_in_each_language() {
        for key in MICROPHONE_HELP {
            for language in LANGUAGES.iter().filter(|language| **language != "en") {
                assert_ne!(t(language, key), t("en", key), "{language} {key}: its own words");
            }
        }
    }

    #[test]
    fn english_is_the_fallback() {
        assert_eq!(t("es", "tray.hang_up"), "Colgar");
        assert_eq!(t("de", "tray.hang_up"), "Auflegen");
        assert_eq!(t("ru", "tray.hang_up"), "Hang up", "a language without a bundle");
        assert_eq!(t("es", "no.such.key"), "no.such.key");
    }
}
