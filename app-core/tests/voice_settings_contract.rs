//! `VoiceSettings` against the voice contract the pinned web bundles.
//!
//! The page starts a call with the settings sidevoice-voice's `js/voice-host.d.ts` declares (`VoiceSettings`,
//! `VoiceSettingsChange`); this crate reads them again in Rust, strictly (`deny_unknown_fields`), so the two are one
//! schema kept in two places until sidevoice-voice#38 is evaluated (D-HOST). This test is what notices that they have
//! drifted. It reads the `.d.ts` of the voice version the pinned web (`web.pin.json`) depends on, not the one this app
//! builds with (the web pins an older sidevoice-voice than the app does), and it asks of every shape the contract
//! allows:
//! - the same members, in Rust as in the contract, for each object (an unknown key is refused, and the refusal names
//!   the members Rust knows);
//! - a document with every member set, once for each variant of each union, is read and written back as it was;
//! - the same document without one optional member, or with a nullable one `null`, is read.
//!
//! What a model choice holds (`VoiceModelChoice` is open in the contract) is the page's and the app's to agree: a
//! `{catalog, model}` object stands for it here.
//!
//! `cargo xtask voice-contract` fetches the `.d.ts` and runs the ignored test; the rest of the file runs everywhere.

#[path = "voice_settings_contract/typescript.rs"]
mod typescript;

use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};
use sidevoice_app_core::voice::{InputSettings, ListeningBar, OutputSettings, SettingsChange, VoiceSettings};
use typescript::{Declarations, Object, Ty};

/// What Rust refuses and the contract allows, each one stated. The test fails when one is read after all (remove it
/// then) and when anything else is refused.
///
/// `listening_bar` is `{quiet?, playing?}` in the contract and both are required here. The page always sends both
/// (sidevoice-web 0.14.0 builds it from a full default), so no call fails today; whether Rust should take one alone
/// is for the owner of the schema.
/// How many variants of each union are tried: the widest one the contract has is three members.
const VARIANTS: usize = 8;

const KNOWN_DEVIATIONS: [(&str, &str); 2] =
    [("absent", "input.listening_bar.quiet"), ("absent", "input.listening_bar.playing")];

/// The members of an object, by name: the contract's, and what `T` says it knows when it is given one it does not.
fn rust_members<T: DeserializeOwned>() -> Vec<String> {
    let refused = serde_json::from_value::<T>(json!({"\u{1}": 0}));
    let message = match refused {
        Ok(_) => return vec!["(takes any key)".into()],
        Err(error) => error.to_string(),
    };
    // "unknown field `\u{1}`, expected one of `a`, `b`", or "expected `a`", or "there are no fields".
    let mut quoted = message.split('`').skip(1).step_by(2).map(str::to_owned);
    quoted.next();
    let mut members: Vec<String> = quoted.collect();
    members.sort();
    members
}

fn contract_members(object: &Object) -> Vec<String> {
    let mut members: Vec<String> = object.fields.iter().map(|field| field.name.clone()).collect();
    members.sort();
    members
}

struct Contract {
    declarations: Declarations,
}

impl Contract {
    /// `ty` with references followed and, for a union, its `variant`th member that is not `null`.
    fn pick(&self, ty: &Ty, variant: usize) -> Result<Ty, String> {
        match self.declarations.resolve(ty)? {
            Ty::Union(members) => {
                let members: Vec<&Ty> = members.iter().filter(|member| **member != Ty::Null).collect();
                self.pick(members[variant % members.len()], variant)
            }
            other => Ok(other),
        }
    }

    fn object(&self, ty: &Ty, variant: usize) -> Result<Object, String> {
        match self.pick(ty, variant)? {
            Ty::Object(object) => Ok(object),
            other => Err(format!("expected an object, not {other:?}")),
        }
    }

    fn nullable(&self, ty: &Ty) -> Result<bool, String> {
        Ok(match self.declarations.resolve(ty)? {
            Ty::Null => true,
            Ty::Union(members) => members.contains(&Ty::Null),
            _ => false,
        })
    }

    /// A value of `ty` with every member set; `variant` picks among the members of each union.
    fn sample(&self, ty: &Ty, variant: usize) -> Result<Value, String> {
        Ok(match self.pick(ty, variant)? {
            Ty::Number => json!(1),
            Ty::String => json!("en"),
            Ty::Boolean => json!(true),
            Ty::Literal(text) => json!(text),
            Ty::Object(object) => {
                let mut map = Map::new();
                if object.open {
                    map.extend([("catalog".to_owned(), json!("local")), ("model".to_owned(), json!("m"))]);
                }
                for field in &object.fields {
                    map.insert(field.name.clone(), self.sample(&field.ty, variant)?);
                }
                Value::Object(map)
            }
            other => return Err(format!("no sample for {other:?}")),
        })
    }

    /// Every way a document of `ty` may differ and still be one: an optional member left out, a nullable one `null`.
    /// Each is (what, path of members) in a document that has all of them.
    fn variations(
        &self,
        ty: &Ty,
        variant: usize,
        path: &mut Vec<String>,
        found: &mut Vec<(String, Vec<String>)>,
    ) -> Result<(), String> {
        for field in &self.object(ty, variant)?.fields {
            path.push(field.name.clone());
            if field.optional {
                found.push(("absent".into(), path.clone()));
            }
            if self.nullable(&field.ty)? {
                found.push(("null".into(), path.clone()));
            }
            if matches!(self.pick(&field.ty, variant)?, Ty::Object(_)) {
                self.variations(&field.ty, variant, path, found)?;
            }
            path.pop();
        }
        Ok(())
    }
}

fn vary(document: &Value, what: &str, path: &[String]) -> Value {
    let mut document = document.clone();
    let (last, parents) = path.split_last().expect("a path");
    let mut at = &mut document;
    for member in parents {
        at = &mut at[member];
    }
    match what {
        "absent" => {
            at.as_object_mut().expect("an object").remove(last);
        }
        _ => at[last] = Value::Null,
    }
    document
}

/// The document as the other side writes it: `null` members are left out and numbers are numbers.
fn plain(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            Value::Object(map.iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.clone(), plain(v))).collect())
        }
        Value::Number(number) => json!(number.as_f64()),
        other => other.clone(),
    }
}

/// What differs between the contract in `source` and the settings this crate reads, one line each; none when they
/// agree.
fn drift(source: &str) -> Result<Vec<String>, String> {
    let contract = Contract { declarations: Declarations::new(source) };
    let mut drift = Vec::new();

    // The same members.
    let input = contract.object(&Ty::Reference("VoiceInputSettings".into()), 0)?;
    let bar = input.fields.iter().find(|field| field.name == "listening_bar").ok_or("no `listening_bar`")?;
    let members = [
        (
            "VoiceSettings",
            contract_members(&contract.object(&Ty::Reference("VoiceSettings".into()), 0)?),
            rust_members::<VoiceSettings>(),
        ),
        (
            "VoiceSettingsChange",
            contract_members(&contract.object(&Ty::Reference("VoiceSettingsChange".into()), 0)?),
            rust_members::<SettingsChange>(),
        ),
        ("VoiceInputSettings", contract_members(&input), rust_members::<InputSettings>()),
        (
            "VoiceInputSettings.listening_bar",
            contract_members(&contract.object(&bar.ty, 0)?),
            rust_members::<ListeningBar>(),
        ),
        (
            "VoiceOutputSettings",
            contract_members(&contract.object(&Ty::Reference("VoiceOutputSettings".into()), 0)?),
            rust_members::<OutputSettings>(),
        ),
    ];
    for (name, wanted, known) in members {
        if wanted != known {
            drift.push(format!("{name}: the contract has {wanted:?}, Rust reads {known:?}"));
        }
    }

    // The documents, and the way each may differ.
    for variant in 0..VARIANTS {
        for (name, change) in [("VoiceSettings", false), ("VoiceSettingsChange", true)] {
            let ty = Ty::Reference(name.into());
            let document = contract.sample(&ty, variant)?;
            let read = |document: &Value| -> Result<Value, String> {
                if change {
                    serde_json::from_value::<SettingsChange>(document.clone())
                        .map(|_| Value::Null)
                        .map_err(|e| e.to_string())
                } else {
                    let settings =
                        serde_json::from_value::<VoiceSettings>(document.clone()).map_err(|e| e.to_string())?;
                    serde_json::to_value(settings).map_err(|e| e.to_string())
                }
            };
            match read(&document) {
                Ok(written) if change || plain(&written) == plain(&document) => {}
                Ok(written) => drift.push(format!("{name} #{variant}: {document} is written back as {written}")),
                Err(error) => drift.push(format!("{name} #{variant}: {document} is refused: {error}")),
            }
            // A document that is not read as it is says all there is to say about the ones that differ from it by a member.
            if drift.iter().any(|line| line.starts_with(&format!("{name} #{variant}: {{"))) {
                continue;
            }
            let mut variations = Vec::new();
            contract.variations(&ty, variant, &mut Vec::new(), &mut variations)?;
            for (what, path) in variations {
                let deviation = KNOWN_DEVIATIONS.contains(&(what.as_str(), path.join(".").as_str()));
                let outcome = read(&vary(&document, &what, &path));
                match (outcome, deviation) {
                    (Err(error), false) => {
                        drift.push(format!("{name} #{variant}: with {} {what}, refused: {error}", path.join(".")))
                    }
                    (Ok(_), true) => drift.push(format!(
                        "{name} #{variant}: {} {what} is read now: remove it from KNOWN_DEVIATIONS",
                        path.join(".")
                    )),
                    _ => {}
                }
            }
        }
    }
    drift.sort();
    drift.dedup();
    Ok(drift)
}

#[test]
#[ignore = "reads the voice contract the pinned web bundles: `cargo xtask voice-contract` fetches it and runs this"]
fn the_settings_are_the_contract_the_pinned_web_bundles() {
    let path = std::env::var("SIDEVOICE_VOICE_CONTRACT")
        .expect("SIDEVOICE_VOICE_CONTRACT names the voice-host.d.ts to read: run `cargo xtask voice-contract`");
    let source = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let drift = drift(&source).unwrap();
    assert!(drift.is_empty(), "the settings differ from {path}:\n{}", drift.join("\n"));
}

/// The contract in miniature, in the shapes the real one uses.
#[cfg(test)]
const MINIATURE: &str = r#"
export interface VoiceModelChoice { [choice: string]: unknown; }
export type VoiceEndOfTurnSettings = { kind: "silence"; ms?: number } | { kind: "smart"; model?: VoiceModelChoice | null };
export interface VoiceInputSettings {
  stt: VoiceModelChoice & { language?: string | null };
  endOfTurn?: VoiceEndOfTurnSettings;
  patience?: "fast" | "normal" | "calm";
  listening_bar?: { quiet: number; playing: number };
  barge_in_min_speech?: number;
  barge_in_window?: number;
  vad_confidence?: number;
  speech_start_ms?: number;
  pre_roll_ms?: number;
}
export interface VoiceOutputSettings {
  tts: VoiceModelChoice;
  voice?: string | null;
  speed?: number;
  fade_ms?: number;
  language?: string | null;
}
export interface VoiceSettings {
  input?: VoiceInputSettings | null;
  output?: VoiceOutputSettings | null;
  audio_grace_ms?: number;
  idle_unload_minutes?: number;
}
export interface VoiceSettingsChange {
  input?: VoiceInputSettings | null;
  output?: VoiceOutputSettings | null;
  audio_grace_ms?: number;
  idle_unload_minutes?: number;
}
"#;

#[test]
fn a_contract_the_settings_follow_has_no_drift() {
    assert_eq!(drift(MINIATURE).unwrap(), Vec::<String>::new());
}

#[test]
fn a_member_the_contract_adds_is_noticed() {
    let added = MINIATURE.replace("  audio_grace_ms?: number;\n  idle_unload_minutes?: number;\n}\nexport interface VoiceSettingsChange", "  audio_grace_ms?: number;\n  idle_unload_minutes?: number;\n  mood?: string;\n}\nexport interface VoiceSettingsChange");
    let found = drift(&added).unwrap();
    assert!(
        found.iter().any(|line| line.starts_with("VoiceSettings: the contract has") && line.contains("mood")),
        "{found:?}"
    );
    assert!(found.iter().any(|line| line.contains("mood") && line.contains("refused")), "{found:?}");
}

#[test]
fn a_member_the_contract_drops_is_noticed() {
    let dropped = MINIATURE.replace("  pre_roll_ms?: number;\n", "");
    let found = drift(&dropped).unwrap();
    assert!(
        found.iter().any(|line| line.starts_with("VoiceInputSettings: ") && line.contains("pre_roll_ms")),
        "{found:?}"
    );
}

#[test]
fn a_variant_the_contract_adds_is_noticed() {
    let widened = MINIATURE.replace(r#""fast" | "normal" | "calm""#, r#""fast" | "normal" | "calm" | "eager""#);
    let found = drift(&widened).unwrap();
    assert!(found.iter().any(|line| line.contains("eager") && line.contains("refused")), "{found:?}");
    let other_kind = MINIATURE
        .replace(r#"{ kind: "silence"; ms?: number }"#, r#"{ kind: "silence"; ms?: number } | { kind: "never" }"#);
    assert!(drift(&other_kind).unwrap().iter().any(|line| line.contains("never") && line.contains("refused")));
}

#[test]
fn a_member_the_contract_makes_optional_but_rust_requires_is_noticed() {
    // `output.tts` is required in both; `voice` is optional in both. Making `tts` optional in the contract is drift.
    let loosened = MINIATURE.replace("  tts: VoiceModelChoice;", "  tts?: VoiceModelChoice;");
    let found = drift(&loosened).unwrap();
    assert!(
        found.iter().any(|line| line.contains("output.tts absent") || line.contains("with output.tts absent")),
        "{found:?}"
    );
}

#[test]
fn the_stated_deviation_is_tolerated_and_nothing_else() {
    // The contract has `listening_bar` members optional, as the real one does; Rust requires both, as stated above.
    let optional = MINIATURE.replace("{ quiet: number; playing: number }", "{ quiet?: number; playing?: number }");
    assert_eq!(drift(&optional).unwrap(), Vec::<String>::new());
    // Any other member made optional there is not stated, so it is drift.
    let loosened = optional.replace("  stt: VoiceModelChoice", "  stt?: VoiceModelChoice");
    assert!(drift(&loosened).unwrap().iter().any(|line| line.contains("input.stt absent")));
}

#[cfg(test)]
mod reader {
    use super::typescript::{Declarations, Field, Object, Ty};

    #[test]
    fn declarations_are_read_when_asked_for() {
        let declarations = Declarations::new(
            "import type { A } from \"./a.js\";\n\
             /** the model */\nexport interface Choice { [key: string]: unknown }\n\
             export interface Thing extends Other<A> { a?: number; // why\n b: \"x\" | \"y\" | null; c: Choice & { d?: string } }\n\
             export interface Host { start(settings: Thing): void; }\n\
             export type Kind = { kind: \"one\" } | { kind: \"two\"; n?: number };",
        );
        let thing = declarations.get("Thing").unwrap();
        let Ty::Object(Object { fields, open: false }) = thing else { panic!("an object") };
        assert_eq!(
            fields,
            [
                Field { name: "a".into(), optional: true, ty: Ty::Number },
                Field {
                    name: "b".into(),
                    optional: false,
                    ty: Ty::Union(vec![Ty::Literal("x".into()), Ty::Literal("y".into()), Ty::Null])
                },
                Field {
                    name: "c".into(),
                    optional: false,
                    ty: Ty::Intersection(vec![
                        Ty::Reference("Choice".into()),
                        Ty::Object(Object {
                            fields: vec![Field { name: "d".into(), optional: true, ty: Ty::String }],
                            open: false
                        })
                    ])
                }
            ]
        );
        assert!(matches!(declarations.get("Kind").unwrap(), Ty::Union(kinds) if kinds.len() == 2));
        let merged = declarations.resolve(&Ty::Reference("Choice".into())).unwrap();
        assert!(matches!(merged, Ty::Object(Object { open: true, .. })));
        assert!(declarations.get("Missing").is_err());
    }
}
