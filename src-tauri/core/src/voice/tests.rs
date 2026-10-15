use super::*;

fn model(id: &str, family: Option<&str>, capabilities: &[&str], runs: bool) -> Candidate {
    Candidate {
        id: id.into(),
        family: family.map(Into::into),
        capabilities: capabilities.iter().map(|c| c.to_string()).collect(),
        runs,
        builds: Vec::new(),
    }
}

/// A local model with `builds`: each an id and why it does not run here (`None`: it runs).
fn built(id: &str, capabilities: &[&str], builds: &[(&str, Option<&str>)]) -> Candidate {
    Candidate {
        builds: builds
            .iter()
            .map(|(id, unfit)| BuildCandidate { id: id.to_string(), unfit: unfit.map(Into::into) })
            .collect(),
        ..model(id, Some(id), capabilities, builds.iter().any(|(_, unfit)| unfit.is_none()))
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
                built(
                    "whisper-small",
                    &["stt"],
                    &[("whisper-small/metal", Some("memory")), ("whisper-small/cpu", None)],
                ),
                model("parakeet", Some("parakeet"), &["stt"], false),
                model("kokoro-82m-v1.0", Some("kokoro"), &["tts"], true),
                model("smart-turn-v3", Some("smart-turn"), &["end-of-turn"], true),
                model("smart-turn-v4", Some("smart-turn"), &["end-of-turn"], false),
                model("smart-turn-v3-int8", Some("smart-turn"), &["end-of-turn"], true),
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
    ModelChoice::local(model)
}

#[test]
fn each_slot_is_a_catalogues_model_and_the_detector_is_the_local_silero() {
    let choice =
        choose(&settings(("openai", "gpt-4o-mini-transcribe"), ("local", "kokoro-82m-v1.0")), &catalogs(true)).unwrap();
    assert_eq!(
        choice.stt,
        ModelChoice { catalog: "openai".into(), model: "gpt-4o-mini-transcribe".into(), build: None }
    );
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
fn barge_in_duration_reaches_the_native_voice_config_without_changing_model_choice() {
    let original = settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0"));
    let mut changed = original.clone();
    changed.barge_in_min_speech = Some(0.8);
    let before = choose(&original, &catalogs(true)).unwrap();
    let after = choose(&changed, &catalogs(true)).unwrap();
    assert!(before.same_models(&after));
    assert_eq!(after.config["barge_in_min_speech"], json!(0.8));
    assert!(before.config.get("barge_in_min_speech").is_none());
}

#[test]
fn smart_turn_takes_the_local_model_that_ends_turns() {
    let mut chosen = settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0"));
    chosen.end_of_turn = Some(EndOfTurn::SmartTurn);
    assert_eq!(choose(&chosen, &catalogs(true)).unwrap().end_of_turn, Some(local("smart-turn-v3")));

    let mut without = catalogs(true);
    without[0].models.retain(|m| !m.capabilities.contains(&"end-of-turn".to_string()));
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
    moved.stt =
        SttChoice { catalog: "openai".into(), model: "gpt-4o-mini-transcribe".into(), language: None, build: None };
    assert!(!first.same_models(&choose(&moved, &catalogs(true)).unwrap()));
    let mut smart = base;
    smart.end_of_turn = Some(EndOfTurn::SmartTurn);
    assert!(!first.same_models(&choose(&smart, &catalogs(true)).unwrap()));
}

#[test]
fn smart_turn_takes_the_end_of_turn_model_chosen_when_it_runs_else_the_first() {
    let mut chosen = settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0"));
    chosen.end_of_turn = Some(EndOfTurn::SmartTurn);
    chosen.turn_model = Some("smart-turn-v3-int8".into());
    assert_eq!(choose(&chosen, &catalogs(true)).unwrap().end_of_turn, Some(local("smart-turn-v3-int8")));
    // One that does not run here, one that does not end turns, one not listed: the first that runs, as on the web.
    for other in ["smart-turn-v4", "silero-vad", "nowhere"] {
        chosen.turn_model = Some(other.into());
        assert_eq!(choose(&chosen, &catalogs(true)).unwrap().end_of_turn, Some(local("smart-turn-v3")), "{other}");
    }
    // Ending turns on silence, a model chosen loads nothing.
    chosen.end_of_turn = Some(EndOfTurn::Silence);
    chosen.turn_model = Some("smart-turn-v3-int8".into());
    assert_eq!(choose(&chosen, &catalogs(true)).unwrap().end_of_turn, None);
}

#[test]
fn a_local_models_build_chosen_is_loaded_when_it_is_one_of_its_builds_that_runs() {
    let mut chosen = settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0"));
    assert_eq!(choose(&chosen, &catalogs(true)).unwrap().stt.build, None, "none chosen: the catalogue picks");
    chosen.stt.build = Some("whisper-small/cpu".into());
    let choice = choose(&chosen, &catalogs(true)).unwrap();
    assert_eq!(choice.stt, ModelChoice { build: Some("whisper-small/cpu".into()), ..local("whisper-small") });
    chosen.stt.build = Some("whisper-small/metal".into());
    let unfit = choose(&chosen, &catalogs(true)).unwrap_err();
    assert_eq!((unfit.code.as_str(), unfit.model.as_str()), ("memory", "whisper-small"), "why it does not run here");
    chosen.stt.build = Some("whisper-tiny/cpu".into());
    assert_eq!(choose(&chosen, &catalogs(true)).unwrap_err().code, "build-not-found", "not a build of this model");
    chosen.stt.build = None;
    chosen.tts.build = Some("kokoro/nope".into());
    assert_eq!(choose(&chosen, &catalogs(true)).unwrap_err().code, "build-not-found");
    // An empty build names none, as on the web: the catalogue picks.
    chosen.stt.build = Some(String::new());
    chosen.tts.build = Some(String::new());
    let choice = choose(&chosen, &catalogs(true)).unwrap();
    assert_eq!((choice.stt.build, choice.tts.build), (None, None));
    // The model is checked first: one with no build that runs is unfit, whichever build is named.
    let mut unfit = settings(("local", "parakeet"), ("local", "kokoro-82m-v1.0"));
    unfit.stt.build = Some("parakeet/nope".into());
    assert_eq!(choose(&unfit, &catalogs(true)).unwrap_err().code, "model-unfit");
    // A provider's model has no builds: one named is not looked at, as on the web.
    let mut remote = settings(("openai", "gpt-4o-mini-transcribe"), ("local", "kokoro-82m-v1.0"));
    remote.stt.build = Some("whatever".into());
    assert_eq!(choose(&remote, &catalogs(true)).unwrap().stt.build, None);
}

#[test]
fn other_models_come_with_another_build_or_end_of_turn_model() {
    let mut base = settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0"));
    base.end_of_turn = Some(EndOfTurn::SmartTurn);
    let first = choose(&base, &catalogs(true)).unwrap();
    let mut built = base.clone();
    built.stt.build = Some("whisper-small/cpu".into());
    let built = choose(&built, &catalogs(true)).unwrap();
    assert!(!first.same_models(&built), "a build chosen");
    let mut turned = base.clone();
    turned.turn_model = Some("smart-turn-v3-int8".into());
    assert!(!first.same_models(&choose(&turned, &catalogs(true)).unwrap()), "another end-of-turn model");
    // Naming the model already taken changes nothing.
    turned.turn_model = Some("smart-turn-v3".into());
    assert!(first.same_models(&choose(&turned, &catalogs(true)).unwrap()));
}

#[test]
fn settings_take_a_build_and_an_end_of_turn_model_and_refuse_any_other_key() {
    let read = |value: Value| serde_json::from_value::<VoiceSettings>(value);
    assert!(read(json!({"stt": {"catalog": "local", "model": "m"}, "tts": {"catalog": "local", "model": "t"}})).is_ok());
    // What the web's settings carry (sidevoice-web `voice-settings.js` DeviceVoiceSettings), every field filled.
    let web = read(json!({
        "stt": {"catalog": "local", "model": "m", "language": null, "build": "m/x"},
        "tts": {"catalog": "local", "model": "t", "voice": null, "speed": 1, "build": "t/y"},
        "patience": "normal", "end_of_turn": "smart-turn", "idle_unload_minutes": 10, "turn_model": "smart-turn-v3",
    }))
    .unwrap();
    assert_eq!(
        (web.stt.build.as_deref(), web.tts.build.as_deref(), web.turn_model.as_deref()),
        (Some("m/x"), Some("t/y"), Some("smart-turn-v3"))
    );
    let unknown = |slot: &str| {
        let mut value = json!({"stt": {"catalog": "local", "model": "m"}, "tts": {"catalog": "local", "model": "t"}});
        match slot {
            "top" => value["nope"] = json!(1),
            slot => value[slot]["nope"] = json!(1),
        }
        read(value).is_err()
    };
    assert!(unknown("top") && unknown("stt") && unknown("tts"), "an unknown key is refused");
    assert!(
        read(json!({"stt": {"model": "m"}, "tts": {"catalog": "local", "model": "t"}})).is_err(),
        "a catalogue is named"
    );
}
