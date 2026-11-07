//! The app's wording of the web's texts that its place changes (`host.messages(language)`, docs/BRIDGE.md -> "The message
//! overlay"): tables of the web catalogue's keys to the app's own text, one per language (`i18n/<language>.json`), `en`
//! among them. What every app shares is here; an app adds its own over it, for the same languages. The loader is
//! sidevoice-shared-i18n's: this file only holds the shared tables.

pub use sidevoice_shared_i18n::{check, LANGUAGES};

/// What every app words the same way, one table per language of [`LANGUAGES`].
pub const SHARED: sidevoice_shared_i18n::Bundles = [
    ("en", include_str!("../i18n/en.json")),
    ("de", include_str!("../i18n/de.json")),
    ("es", include_str!("../i18n/es.json")),
    ("fr", include_str!("../i18n/fr.json")),
    ("it", include_str!("../i18n/it.json")),
    ("ja", include_str!("../i18n/ja.json")),
    ("pl", include_str!("../i18n/pl.json")),
    ("zh-Hans", include_str!("../i18n/zh-Hans.json")),
    ("pt-BR", include_str!("../i18n/pt-BR.json")),
    ("ko", include_str!("../i18n/ko.json")),
    ("hi", include_str!("../i18n/hi.json")),
];

/// The overlay the page is given: `{ "<language>": { "<key>": text } }` for every language of [`LANGUAGES`], the shared
/// texts with the app's `own` (a language and its JSON table each) over them.
pub fn tables(own: &[(&str, &str)]) -> serde_json::Value {
    sidevoice_shared_i18n::overlay(&SHARED, own)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_texts_are_in_every_language_with_english_s_keys() {
        assert_eq!(check(&SHARED), Ok(()));
        assert!(check(&SHARED[1..]).is_err());
    }

    #[test]
    fn an_app_s_own_texts_come_over_the_shared_ones_in_their_language() {
        let own = [("es", r#"{"a":"propio","settings.call.keepAwakeUnsupported":"otro"}"#)];
        let tables = tables(&own);
        assert_eq!(tables["es"]["a"], "propio");
        assert_eq!(tables["es"]["settings.call.keepAwakeUnsupported"], "otro");
        assert_eq!(tables["en"].get("a"), None);
        assert_eq!(tables.as_object().map(|all| all.len()), Some(LANGUAGES.len()));
    }
}
