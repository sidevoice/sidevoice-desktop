use super::*;

fn model(id: &str, family: Option<&str>, capabilities: &[&str], runs: bool) -> Candidate {
    Candidate {
        id: id.into(),
        family: family.map(Into::into),
        capabilities: capabilities.iter().map(|c| c.to_string()).collect(),
        runs,
    }
}

/// The local catalogue, and OpenAI with its listing (`keyed`) or without a key.
fn catalogs(keyed: bool) -> Vec<CatalogCandidates> {
    vec![
        CatalogCandidates {
            id: LOCAL_CATALOG.into(),
            reason: None,
            detail: None,
            models: vec![
                model("other-vad", Some("other-vad"), &["vad"], true),
                model("silero-vad", Some("silero-vad"), &["vad"], true),
                model("whisper-small", Some("whisper"), &["stt"], true),
                model("parakeet", Some("parakeet"), &["stt"], false),
                model("kokoro-82m-v1.0", Some("kokoro"), &["tts"], true),
                model("smart-turn-v3", Some("smart-turn"), &["end-of-turn"], true),
            ],
        },
        CatalogCandidates {
            id: "openai".into(),
            reason: (!keyed).then(|| "credential-missing".into()),
            detail: (!keyed).then(|| "401: no key".into()),
            models: if keyed { vec![model("gpt-4o-mini-transcribe", None, &["stt"], true)] } else { Vec::new() },
        },
    ]
}

fn settings(stt: (&str, &str), tts: (&str, &str)) -> VoiceSettings {
    serde_json::from_value(json!({
        "stt": {"catalog": stt.0, "model": stt.1, "language": "es"},
        "tts": {"catalog": tts.0, "model": tts.1, "voice": "ef_dora", "speed": 1.2},
        "patience": "calm",
        "idle_unload_minutes": 30,
    }))
    .unwrap()
}

fn local(model: &str) -> ModelChoice {
    ModelChoice { catalog: LOCAL_CATALOG.into(), model: model.into() }
}

#[test]
fn each_slot_is_a_catalogues_model_and_the_detector_is_the_local_silero() {
    let choice =
        choose(&settings(("openai", "gpt-4o-mini-transcribe"), ("local", "kokoro-82m-v1.0")), &catalogs(true)).unwrap();
    assert_eq!(choice.stt, ModelChoice { catalog: "openai".into(), model: "gpt-4o-mini-transcribe".into() });
    assert_eq!(choice.tts, local("kokoro-82m-v1.0"));
    assert_eq!(choice.vad, local("silero-vad"), "Silero first, wherever it is listed");
    assert_eq!(choice.end_of_turn, None);
    assert_eq!(
        choice.config,
        json!({"language": "es", "voice": "ef_dora", "speed": 1.2f32, "end_of_turn": "silence", "patience": "calm",
            "idle_unload_minutes": 30})
    );
}

#[test]
fn smart_turn_takes_the_local_model_that_ends_turns() {
    let mut chosen = settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0"));
    chosen.end_of_turn = Some(EndOfTurn::SmartTurn);
    assert_eq!(choose(&chosen, &catalogs(true)).unwrap().end_of_turn, Some(local("smart-turn-v3")));

    let mut without = catalogs(true);
    without[0].models.retain(|m| m.id != "smart-turn-v3");
    assert_eq!(choose(&chosen, &without).unwrap_err().code, "end-of-turn-unavailable");
}

#[test]
fn a_slot_the_catalogues_cannot_fill_is_refused_by_code() {
    let refused = |stt: (&str, &str), catalogs: &[CatalogCandidates]| {
        choose(&settings(stt, ("local", "kokoro-82m-v1.0")), catalogs).unwrap_err()
    };
    assert_eq!(refused(("nowhere", "whisper-small"), &catalogs(true)).code, "catalog-not-found");
    assert_eq!(refused(("local", "no-such-model"), &catalogs(true)).code, "model-unknown");
    // Listed, but not for this task.
    assert_eq!(refused(("local", "kokoro-82m-v1.0"), &catalogs(true)).code, "model-unknown");
    assert_eq!(refused(("local", "parakeet"), &catalogs(true)).code, "model-unfit");
    // A provider that lists nothing says why, with the provider's own words.
    let keyless = refused(("openai", "gpt-4o-mini-transcribe"), &catalogs(false));
    assert_eq!((keyless.code.as_str(), keyless.detail.as_deref()), ("credential-missing", Some("401: no key")));

    let mut no_detector = catalogs(true);
    no_detector[0].models.retain(|m| !m.capabilities.contains(&"vad".to_string()));
    assert_eq!(refused(("local", "whisper-small"), &no_detector).code, "vad-unavailable");
}

#[test]
fn another_local_detector_runs_when_there_is_no_silero() {
    let mut catalogs = catalogs(true);
    catalogs[0].models.retain(|m| m.id != "silero-vad");
    let choice = choose(&settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0")), &catalogs).unwrap();
    assert_eq!(choice.vad, local("other-vad"));
}

#[test]
fn other_models_come_with_a_slots_catalogue_or_model_or_the_end_of_turn() {
    let base = settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0"));
    let first = choose(&base, &catalogs(true)).unwrap();
    let mut live = base.clone();
    live.stt.language = Some("en".into());
    live.tts.voice = Some("af_heart".into());
    live.tts.speed = Some(0.8);
    live.patience = Some(Patience::Fast);
    assert!(first.same_models(&choose(&live, &catalogs(true)).unwrap()), "language, voice, speed, patience apply live");
    let mut moved = base.clone();
    moved.stt = SttChoice { catalog: "openai".into(), model: "gpt-4o-mini-transcribe".into(), language: None };
    assert!(!first.same_models(&choose(&moved, &catalogs(true)).unwrap()));
    let mut smart = base;
    smart.end_of_turn = Some(EndOfTurn::SmartTurn);
    assert!(!first.same_models(&choose(&smart, &catalogs(true)).unwrap()));
}

#[test]
fn settings_name_a_catalogue_and_take_no_build() {
    let read = |value: Value| serde_json::from_value::<VoiceSettings>(value);
    assert!(read(json!({"stt": {"catalog": "local", "model": "m"}, "tts": {"catalog": "local", "model": "t"}})).is_ok());
    let with_build =
        json!({"stt": {"catalog": "local", "model": "m", "build": "m/x"}, "tts": {"catalog": "local", "model": "t"}});
    assert!(read(with_build).is_err(), "no build any more");
    assert!(
        read(json!({"stt": {"model": "m"}, "tts": {"catalog": "local", "model": "t"}})).is_err(),
        "a catalogue is named"
    );
}
