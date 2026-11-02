//! The app's wording of the web's texts that its place changes (`host.messages(language)`, docs/BRIDGE.md -> "The message
//! overlay"): tables of the web catalogue's keys to the app's own text, one per language (`i18n/<language>.json`), `en`
//! among them. What every app shares is here; an app adds its own over it, for the same languages.

/// The languages the app words its texts in, as the web offers them.
pub const LANGUAGES: [&str; 8] = ["en", "de", "es", "fr", "it", "ja", "pl", "zh-Hans"];

/// What every app words the same way, one table per language of [`LANGUAGES`].
pub const SHARED: [(&str, &str); 8] = [
    ("en", include_str!("../i18n/en.json")),
    ("de", include_str!("../i18n/de.json")),
    ("es", include_str!("../i18n/es.json")),
    ("fr", include_str!("../i18n/fr.json")),
    ("it", include_str!("../i18n/it.json")),
    ("ja", include_str!("../i18n/ja.json")),
    ("pl", include_str!("../i18n/pl.json")),
    ("zh-Hans", include_str!("../i18n/zh-Hans.json")),
];

fn table(json: &str) -> serde_json::Map<String, serde_json::Value> {
    match serde_json::from_str(json) {
        Ok(serde_json::Value::Object(table)) => table,
        // The tables are embedded and tested: this is a build that was not.
        _ => serde_json::Map::new(),
    }
}

/// The overlay the page is given: `{ "<language>": { "<key>": text } }` for every language of [`LANGUAGES`], the shared
/// texts with the app's `own` (a language and its JSON table each) over them.
pub fn tables(own: &[(&str, &str)]) -> serde_json::Value {
    SHARED
        .iter()
        .map(|&(language, shared)| {
            let mut texts = table(shared);
            for (_, json) in own.iter().filter(|(name, _)| *name == language) {
                texts.extend(table(json));
            }
            (language.to_owned(), serde_json::Value::Object(texts))
        })
        .collect::<serde_json::Map<_, _>>()
        .into()
}

/// Whether `bundle` is well made: a JSON table of non-empty strings for each of [`LANGUAGES`] and no other, every one with
/// English's keys. The apps' tests hold their own bundles to it.
pub fn check(bundle: &[(&str, &str)]) -> Result<(), String> {
    let languages: Vec<_> = bundle.iter().map(|(language, _)| *language).collect();
    if languages != LANGUAGES {
        return Err(format!("languages {languages:?}, not {LANGUAGES:?}"));
    }
    let keys = |json: &str| -> Result<Vec<String>, String> {
        let parsed: serde_json::Value = serde_json::from_str(json).map_err(|error| error.to_string())?;
        let texts = parsed.as_object().ok_or("not a table")?;
        for (key, text) in texts {
            if !text.as_str().is_some_and(|text| !text.is_empty()) {
                return Err(format!("{key} is not a text"));
            }
        }
        let mut keys: Vec<String> = texts.keys().cloned().collect();
        keys.sort();
        Ok(keys)
    };
    let english = keys(bundle[0].1).map_err(|error| format!("en: {error}"))?;
    if english.is_empty() {
        return Err("en has no texts".to_owned());
    }
    for (language, json) in bundle {
        if keys(json).map_err(|error| format!("{language}: {error}"))? != english {
            return Err(format!("{language} has other keys than English"));
        }
    }
    Ok(())
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
