//! The mobile app's own texts, the ones the page shows but the app words (docs/MOBILE.md -> "The bridge"): how to allow a
//! blocked microphone on this phone, and the button that opens that settings page (D-HOST-OS: each app words its own
//! system's help). One bundle per language (`i18n/<language>.rs`), English the fallback; adding a text means adding its
//! key to English.
mod en;
mod es;

/// The languages the app words its texts in, with their bundles.
const BUNDLES: [(&str, &[(&str, &str)]); 2] = [("en", en::TEXTS), ("es", es::TEXTS)];

/// The languages the app words its texts in.
pub const LANGUAGES: [&str; 2] = [BUNDLES[0].0, BUNDLES[1].0];

fn find(bundle: &[(&str, &'static str)], key: &str) -> Option<&'static str> {
    bundle
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, text)| *text)
}

/// `key` in `language`, else in English; the key itself when English lacks it too (a test keeps that from happening).
pub fn t(language: &str, key: &'static str) -> &'static str {
    let bundle = BUNDLES
        .iter()
        .find(|(name, _)| *name == language)
        .map_or(en::TEXTS, |(_, bundle)| *bundle);
    find(bundle, key)
        .or_else(|| find(en::TEXTS, key))
        .unwrap_or(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_has_every_english_key_and_no_other() {
        for (language, bundle) in BUNDLES {
            for (key, _) in en::TEXTS {
                assert!(find(bundle, key).is_some(), "{language} lacks {key}");
            }
            for (key, _) in bundle {
                assert!(
                    find(en::TEXTS, key).is_some(),
                    "{language} has {key}, which English does not"
                );
            }
        }
    }

    #[test]
    fn an_unknown_language_is_english() {
        assert_eq!(t("de", "microphone.open_settings_ios"), "Open Settings");
        assert_eq!(t("es", "microphone.open_settings_ios"), "Abrir Ajustes");
    }
}
