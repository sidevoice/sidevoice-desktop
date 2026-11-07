//! The one loader of the apps' texts (AGENTS.md -> "Language of the code and of the product"): a JSON table per language
//! (`{ "<key>": text }`), the eleven the web offers, English the fallback for a key or a language that lacks one. What
//! the apps share and what each app words itself (the desktop's tray, the phone's microphone help, the overlay of the web's
//! texts) are all read here, with their `include_str!` tables; nobody else parses a table.
//!
//! This crate is its own because both the desktop's pure-logic crate (no Tauri, no engine) and the Tauri plugin need it:
//! it names `serde_json` and nothing else, so neither pays for the other.

use std::collections::HashMap;
use std::sync::OnceLock;

/// The languages the apps word their texts in, as the web offers them.
pub const LANGUAGES: [&str; 11] = ["en", "de", "es", "fr", "it", "ja", "pl", "zh-Hans", "pt-BR", "ko", "hi"];

/// The tables of one set of texts: a language and its JSON each, in [`LANGUAGES`]' order.
pub type Bundles = [(&'static str, &'static str); 11];

/// A JSON table read: empty when it is not a table (the tables are embedded and tested; this is a build that was not).
pub fn table(json: &str) -> serde_json::Map<String, serde_json::Value> {
    match serde_json::from_str(json) {
        Ok(serde_json::Value::Object(table)) => table,
        _ => serde_json::Map::new(),
    }
}

/// The overlay the page is given: `{ "<language>": { "<key>": text } }` for every language of [`LANGUAGES`], the `shared`
/// texts with the app's `own` (a language and its JSON table each) over them.
pub fn overlay(shared: &[(&str, &str)], own: &[(&str, &str)]) -> serde_json::Value {
    shared
        .iter()
        .map(|&(language, json)| {
            let mut texts = table(json);
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

/// The first of `preferred` (BCP 47 tags, the person's order) that has a bundle; English when none does. A tag matches by
/// its language (`de-DE` and `de_AT` are `de`; every `pt` is `pt-BR`); Chinese by its script, as the web names it: `zh`, `zh-CN`, `zh-SG` and
/// `zh-Hans-*` are `zh-Hans`, while `zh-Hant`, `zh-TW`, `zh-HK` and `zh-MO` (traditional) have no bundle, so the next
/// preference wins.
pub fn language<S: AsRef<str>>(preferred: &[S]) -> &'static str {
    preferred
        .iter()
        .find_map(|tag| {
            let tag = tag.as_ref().to_ascii_lowercase();
            let mut parts = tag.split(['-', '_']);
            let primary = parts.next()?;
            if primary == "zh" {
                let rest: Vec<_> = parts.collect();
                // The script decides when the tag has one; otherwise the region does.
                let traditional =
                    !rest.contains(&"hans") && rest.iter().any(|part| matches!(*part, "hant" | "tw" | "hk" | "mo"));
                return (!traditional).then_some("zh-Hans");
            }
            // Portuguese is offered in its Brazilian wording only: `pt`, `pt-BR` and `pt-PT` are all `pt-BR` until a
            // European variant exists.
            if primary == "pt" {
                return Some("pt-BR");
            }
            LANGUAGES.iter().copied().find(|code| *code == primary)
        })
        .unwrap_or("en")
}

/// One set of texts, read once from its [`Bundles`]; put it in a `static`.
pub struct Texts {
    bundles: &'static Bundles,
    parsed: OnceLock<HashMap<&'static str, HashMap<String, String>>>,
}

impl Texts {
    pub const fn new(bundles: &'static Bundles) -> Self {
        Self { bundles, parsed: OnceLock::new() }
    }

    /// `key` in `language`, else in English; none when English lacks it too (a test keeps that from happening).
    pub fn get(&self, language: &str, key: &str) -> Option<&str> {
        let parsed = self.parsed.get_or_init(|| {
            self.bundles
                .iter()
                .map(|&(language, json)| {
                    let texts =
                        table(json).into_iter().filter_map(|(key, text)| Some((key, text.as_str()?.to_owned())));
                    (language, texts.collect())
                })
                .collect()
        });
        [language, "en"].into_iter().find_map(|language| parsed.get(language)?.get(key)).map(String::as_str)
    }

    /// The bundles' own tables, to check them (see [`check`]).
    pub fn bundles(&self) -> &'static Bundles {
        self.bundles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static BUNDLES: Bundles = [
        ("en", r#"{"a":"one","b":"two"}"#),
        ("de", r#"{"a":"eins"}"#),
        ("es", "{}"),
        ("fr", "{}"),
        ("it", "{}"),
        ("ja", "{}"),
        ("pl", "{}"),
        ("zh-Hans", "{}"),
        ("pt-BR", "{}"),
        ("ko", "{}"),
        ("hi", "{}"),
    ];
    static TEXTS: Texts = Texts::new(&BUNDLES);

    #[test]
    fn a_text_is_the_language_s_else_english_s_else_none() {
        assert_eq!(TEXTS.get("de", "a"), Some("eins"));
        assert_eq!(TEXTS.get("de", "b"), Some("two"));
        assert_eq!(TEXTS.get("xx", "a"), Some("one"));
        assert_eq!(TEXTS.get("de", "c"), None);
    }

    #[test]
    fn the_first_preferred_language_with_a_bundle_wins() {
        assert_eq!(language(&["de-DE", "en-US"]), "de");
        assert_eq!(language(&["ru-RU", "es_419", "fr"]), "es");
        assert_eq!(language(&["ru", "vi"]), "en");
        assert_eq!(language::<&str>(&[]), "en");
        assert_eq!(language(&["JA-JP"]), "ja");
    }

    #[test]
    fn portuguese_korean_and_hindi_are_matched_by_their_language() {
        for tag in ["pt", "pt-BR", "pt_BR.UTF-8", "pt-PT", "PT-pt"] {
            assert_eq!(language(&[tag]), "pt-BR", "{tag}");
        }
        assert_eq!(language(&["ko-KR"]), "ko");
        assert_eq!(language(&["hi-IN"]), "hi");
        assert_eq!(language(&["ru", "pt-PT", "de"]), "pt-BR");
    }

    #[test]
    fn chinese_is_matched_by_its_script() {
        for tag in ["zh", "zh-CN", "zh_SG", "zh-Hans", "zh-Hans-CN", "zh-Hans-HK"] {
            assert_eq!(language(&[tag]), "zh-Hans", "{tag}");
        }
        for tag in ["zh-Hant", "zh-TW", "zh-Hant-TW", "zh-HK", "zh-MO"] {
            assert_eq!(language(&[tag, "de"]), "de", "{tag} is traditional: no bundle, the next preference");
        }
    }

    #[test]
    fn an_overlay_puts_the_app_s_texts_over_the_shared_ones_in_their_language() {
        let own = [("de", r#"{"a":"eigen","c":"neu"}"#)];
        let all = overlay(&BUNDLES, &own);
        assert_eq!(all["de"]["a"], "eigen");
        assert_eq!(all["de"]["c"], "neu");
        assert_eq!(all["en"]["a"], "one");
        assert_eq!(all.as_object().map(|all| all.len()), Some(LANGUAGES.len()));
    }

    #[test]
    fn check_wants_every_language_with_english_s_keys_as_non_empty_texts() {
        assert!(check(&BUNDLES).is_err(), "the other languages lack English's keys");
        let full: Bundles = BUNDLES.map(|(language, _)| (language, r#"{"a":"x"}"#));
        assert_eq!(check(&full), Ok(()));
        assert!(check(&full[1..]).is_err());
        let empty: Bundles = BUNDLES.map(|(language, _)| (language, r#"{"a":""}"#));
        assert!(check(&empty).is_err());
    }
}
