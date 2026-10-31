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
        "input": {
            "stt": {"catalog": stt.0, "model": stt.1, "language": "es"},
            "patience": "calm",
        },
        "output": {
            "tts": {"catalog": tts.0, "model": tts.1},
            "voice": "ef_dora", "speed": 1.2,
        },
        "idle_unload_minutes": 30,
    }))
    .unwrap()
}

fn local(model: &str) -> ModelChoice {
    ModelChoice::local(model)
}

fn input(choice: &VoiceChoice) -> &InputChoice {
    choice.input.as_ref().expect("an input half")
}

fn output(choice: &VoiceChoice) -> &OutputChoice {
    choice.output.as_ref().expect("an output half")
}

fn smart(settings: &mut VoiceSettings, model: Option<&str>) {
    settings.input.as_mut().unwrap().end_of_turn = Some(EndOfTurn::Smart {
        model: model.map(|model| ModelRef { catalog: LOCAL_CATALOG.into(), model: model.into() }),
    });
}

#[test]
fn each_slot_is_a_catalogues_model_and_the_detector_is_the_local_silero() {
    let choice =
        choose(&settings(("openai", "gpt-4o-mini-transcribe"), ("local", "kokoro-82m-v1.0")), &catalogs(true)).unwrap();
    assert_eq!(
        input(&choice).stt,
        ModelChoice { catalog: "openai".into(), model: "gpt-4o-mini-transcribe".into(), build: None }
    );
    assert_eq!(output(&choice).tts, local("kokoro-82m-v1.0"));
    assert_eq!(input(&choice).vad, local("silero-vad"), "Silero first, wherever it is listed");
    assert_eq!(input(&choice).end_of_turn, None);
    assert_eq!(
        input(&choice).config,
        json!({"language": "es", "end_of_turn": {"kind": "silence"}, "patience": "calm"})
    );
    assert_eq!(output(&choice).config, json!({"language": null, "voice": "ef_dora", "speed": 1.2f32}));
    assert_eq!(choice.config, json!({"idle_unload_minutes": 30}));
}

#[test]
fn a_half_that_is_absent_has_no_slot_and_asks_nothing_of_the_catalogues() {
    // Output only: no detector is needed, so a device with none still speaks.
    let mut listening_only = catalogs(true);
    listening_only[0].models.retain(|m| !m.capabilities.contains(&"vad".to_string()));
    let mut speaks = settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0"));
    speaks.input = None;
    let choice = choose(&speaks, &listening_only).unwrap();
    assert_eq!((choice.input.is_none(), output(&choice).tts.clone()), (true, local("kokoro-82m-v1.0")));

    // Input only: the speaker may be anything, listed or not.
    let mut hears = settings(("local", "whisper-small"), ("nowhere", "nothing"));
    hears.output = None;
    let choice = choose(&hears, &catalogs(true)).unwrap();
    assert_eq!((input(&choice).stt.clone(), choice.output), (local("whisper-small"), None));

    // A half that is null in the page's JSON is absent.
    let read: VoiceSettings = serde_json::from_value(json!({
        "input": null, "output": {"tts": {"catalog": "local", "model": "kokoro-82m-v1.0"}},
    }))
    .unwrap();
    assert_eq!(read.input, None);

    // Neither half is no call.
    hears.input = None;
    assert_eq!(choose(&hears, &catalogs(true)).unwrap_err().code, "halves-missing");
}

#[test]
fn silence_ends_a_turn_with_its_own_pause_and_loads_no_end_of_turn_model() {
    let mut chosen = settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0"));
    chosen.input.as_mut().unwrap().end_of_turn = Some(EndOfTurn::Silence { ms: Some(1500) });
    let choice = choose(&chosen, &catalogs(true)).unwrap();
    assert_eq!(input(&choice).end_of_turn, None);
    assert_eq!(input(&choice).config["end_of_turn"], json!({"kind": "silence", "ms": 1500}));
    // Even where no model that ends turns runs.
    let mut without = catalogs(true);
    without[0].models.retain(|m| !m.capabilities.contains(&"end-of-turn".to_string()));
    assert!(choose(&chosen, &without).is_ok());
}

#[test]
fn barge_in_duration_reaches_the_native_voice_config_without_changing_model_choice() {
    let original = settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0"));
    let mut changed = original.clone();
    changed.input.as_mut().unwrap().barge_in_min_speech = Some(0.8);
    let before = choose(&original, &catalogs(true)).unwrap();
    let after = choose(&changed, &catalogs(true)).unwrap();
    assert!(input(&before).same_models(input(&after)));
    assert_eq!(input(&after).config["barge_in_min_speech"], json!(0.8f32));
    assert!(input(&before).config.get("barge_in_min_speech").is_none());
}

#[test]
fn smart_takes_the_local_model_that_ends_turns() {
    let mut chosen = settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0"));
    smart(&mut chosen, None);
    let choice = choose(&chosen, &catalogs(true)).unwrap();
    assert_eq!(input(&choice).end_of_turn, Some(local("smart-turn-v3")));
    assert_eq!(input(&choice).config["end_of_turn"], json!({"kind": "smart"}));

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

    // The output half's slot is refused too.
    let speaker =
        |tts: (&str, &str)| choose(&settings(("local", "whisper-small"), tts), &catalogs(true)).unwrap_err().code;
    assert_eq!(speaker(("local", "whisper-small")), "model-unknown", "a transcriber does not speak");
    assert_eq!(speaker(("nowhere", "x")), "catalog-not-found");
}

#[test]
fn another_local_detector_runs_when_there_is_no_silero() {
    let mut catalogs = catalogs(true);
    catalogs[0].models.retain(|m| m.id != "silero-vad");
    let choice = choose(&settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0")), &catalogs).unwrap();
    assert_eq!(input(&choice).vad, local("other-vad"));
}

#[test]
fn a_half_keeps_its_models_unless_its_slots_change() {
    let base = settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0"));
    let first = choose(&base, &catalogs(true)).unwrap();
    let mut live = base.clone();
    live.input.as_mut().unwrap().stt.language = Some("en".into());
    live.input.as_mut().unwrap().patience = Some(Patience::Fast);
    live.input.as_mut().unwrap().end_of_turn = Some(EndOfTurn::Silence { ms: Some(900) });
    let output_settings = live.output.as_mut().unwrap();
    output_settings.voice = Some("af_heart".into());
    output_settings.speed = Some(0.8);
    output_settings.language = Some("en".into());
    let live = choose(&live, &catalogs(true)).unwrap();
    assert!(input(&first).same_models(input(&live)), "language, patience, the pause apply live");
    assert!(output(&first).same_models(output(&live)), "voice, speed, language apply live");

    let mut moved = base.clone();
    moved.input.as_mut().unwrap().stt =
        SttChoice { catalog: "openai".into(), model: "gpt-4o-mini-transcribe".into(), language: None, build: None };
    assert!(!input(&first).same_models(input(&choose(&moved, &catalogs(true)).unwrap())));
    let mut smarter = base.clone();
    smart(&mut smarter, None);
    assert!(!input(&first).same_models(input(&choose(&smarter, &catalogs(true)).unwrap())));
}

#[test]
fn smart_takes_the_end_of_turn_model_chosen_when_it_runs_else_the_first() {
    let mut chosen = settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0"));
    smart(&mut chosen, Some("smart-turn-v3-int8"));
    assert_eq!(input(&choose(&chosen, &catalogs(true)).unwrap()).end_of_turn, Some(local("smart-turn-v3-int8")));
    // One that does not run here, one that does not end turns, one not listed: the first that runs, as on the web.
    for other in ["smart-turn-v4", "silero-vad", "nowhere"] {
        smart(&mut chosen, Some(other));
        assert_eq!(
            input(&choose(&chosen, &catalogs(true)).unwrap()).end_of_turn,
            Some(local("smart-turn-v3")),
            "{other}"
        );
    }
    // Another catalogue's model of that name is not the local one.
    chosen.input.as_mut().unwrap().end_of_turn = Some(EndOfTurn::Smart {
        model: Some(ModelRef { catalog: "openai".into(), model: "smart-turn-v3-int8".into() }),
    });
    assert_eq!(input(&choose(&chosen, &catalogs(true)).unwrap()).end_of_turn, Some(local("smart-turn-v3")));
}

#[test]
fn a_local_models_build_chosen_is_loaded_when_it_is_one_of_its_builds_that_runs() {
    let mut chosen = settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0"));
    fn stt(settings: &mut VoiceSettings) -> &mut SttChoice {
        &mut settings.input.as_mut().unwrap().stt
    }
    fn tts(settings: &mut VoiceSettings) -> &mut TtsChoice {
        &mut settings.output.as_mut().unwrap().tts
    }
    assert_eq!(input(&choose(&chosen, &catalogs(true)).unwrap()).stt.build, None, "none chosen: the catalogue picks");
    stt(&mut chosen).build = Some("whisper-small/cpu".into());
    let choice = choose(&chosen, &catalogs(true)).unwrap();
    assert_eq!(input(&choice).stt, ModelChoice { build: Some("whisper-small/cpu".into()), ..local("whisper-small") });
    stt(&mut chosen).build = Some("whisper-small/metal".into());
    let unfit = choose(&chosen, &catalogs(true)).unwrap_err();
    assert_eq!((unfit.code.as_str(), unfit.model.as_str()), ("memory", "whisper-small"), "why it does not run here");
    stt(&mut chosen).build = Some("whisper-tiny/cpu".into());
    assert_eq!(choose(&chosen, &catalogs(true)).unwrap_err().code, "build-not-found", "not a build of this model");
    stt(&mut chosen).build = None;
    tts(&mut chosen).build = Some("kokoro/nope".into());
    assert_eq!(choose(&chosen, &catalogs(true)).unwrap_err().code, "build-not-found");
    // An empty build names none, as on the web: the catalogue picks.
    stt(&mut chosen).build = Some(String::new());
    tts(&mut chosen).build = Some(String::new());
    let choice = choose(&chosen, &catalogs(true)).unwrap();
    assert_eq!((input(&choice).stt.build.clone(), output(&choice).tts.build.clone()), (None, None));
    // The model is checked first: one with no build that runs is unfit, whichever build is named.
    let mut unfit = settings(("local", "parakeet"), ("local", "kokoro-82m-v1.0"));
    stt(&mut unfit).build = Some("parakeet/nope".into());
    assert_eq!(choose(&unfit, &catalogs(true)).unwrap_err().code, "model-unfit");
    // A provider's model has no builds: one named is not looked at, as on the web.
    let mut remote = settings(("openai", "gpt-4o-mini-transcribe"), ("local", "kokoro-82m-v1.0"));
    stt(&mut remote).build = Some("whatever".into());
    assert_eq!(input(&choose(&remote, &catalogs(true)).unwrap()).stt.build, None);
}

#[test]
fn settings_take_the_seams_shape_and_refuse_any_other_key() {
    let read = |value: Value| serde_json::from_value::<VoiceSettings>(value);
    assert!(read(json!({"input": {"stt": {"catalog": "local", "model": "m"}}})).is_ok());
    assert!(read(json!({"output": {"tts": {"catalog": "local", "model": "t"}}})).is_ok());
    // What the web's settings carry for a call, every field filled.
    let web = read(json!({
        "input": {
            "stt": {"catalog": "local", "model": "m", "language": null, "build": "m/x"},
            "endOfTurn": {"kind": "smart", "model": {"catalog": "local", "model": "smart-turn-v3"}},
            "patience": "normal", "barge_in_min_speech": 1.0,
        },
        "output": {
            "tts": {"catalog": "local", "model": "t", "build": "t/y"}, "voice": null, "speed": 1, "language": "en",
        },
        "idle_unload_minutes": 10,
    }))
    .unwrap();
    let (stt, tts) = (&web.input.as_ref().unwrap().stt, &web.output.as_ref().unwrap().tts);
    assert_eq!((stt.build.as_deref(), tts.build.as_deref()), (Some("m/x"), Some("t/y")));
    let unknown = |at: &[&str]| {
        let mut value = json!({
            "input": {"stt": {"catalog": "local", "model": "m"}, "endOfTurn": {"kind": "smart", "model": {"catalog": "local", "model": "e"}}},
            "output": {"tts": {"catalog": "local", "model": "t"}},
        });
        let mut at_key = &mut value;
        for key in at {
            at_key = &mut at_key[*key];
        }
        at_key["nope"] = json!(1);
        read(value).is_err()
    };
    assert!(unknown(&[]) && unknown(&["input"]) && unknown(&["input", "stt"]) && unknown(&["output"]));
    assert!(
        unknown(&["output", "tts"]) && unknown(&["input", "endOfTurn"]) && unknown(&["input", "endOfTurn", "model"])
    );
    assert!(read(json!({"input": {"stt": {"model": "m"}}})).is_err(), "a catalogue is named");
    assert!(read(json!({"input": {"stt": {"catalog": "local", "model": "m"}, "endOfTurn": {"kind": "loud"}}})).is_err());
}

#[test]
fn a_change_replaces_adds_or_removes_a_half_and_leaves_the_rest() {
    let both = settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0"));
    let change = |value: Value| serde_json::from_value::<SettingsChange>(value).unwrap();

    // Nothing named stays.
    assert_eq!(both.changed(change(json!({}))), both);
    // `null` removes a half.
    let listening = both.changed(change(json!({"output": null})));
    assert_eq!((listening.input.clone(), listening.output.clone()), (both.input.clone(), None));
    // Settings add it back.
    let back = listening.changed(change(json!({"output": {"tts": {"catalog": "local", "model": "kokoro-82m-v1.0"}}})));
    assert_eq!(back.output.as_ref().map(|o| o.tts.model.as_str()), Some("kokoro-82m-v1.0"));
    assert_eq!(back.input, both.input, "the input half was not named");
    // Settings replace one that is there, whole.
    let replaced = both.changed(change(json!({"input": {"stt": {"catalog": "local", "model": "parakeet"}}})));
    assert_eq!(replaced.input.as_ref().unwrap().patience, None);
    assert_eq!(replaced.output, both.output);
    // The idle minutes are the call's.
    assert_eq!(both.changed(change(json!({"idle_unload_minutes": 0}))).idle_unload_minutes, Some(0));
    assert_eq!(both.changed(change(json!({"output": null}))).idle_unload_minutes, Some(30));

    // Removing the last half is a change the call cannot make: choosing refuses it.
    let none = listening.changed(change(json!({"input": null})));
    assert_eq!(choose(&none, &catalogs(true)).unwrap_err().code, "halves-missing");
    assert!(serde_json::from_value::<SettingsChange>(json!({"nope": 1})).is_err());
}

/// What the web sends for a call with its defaults (sidevoice-web `voice-settings.js`, `voiceHalves` of
/// `defaultVoiceSettings`, `TURN_TUNING`): every turn number it owns, at the input half, the output half and the call.
fn web_defaults() -> Value {
    json!({
        "input": {
            "stt": {"catalog": "local", "model": "whisper-small", "language": null},
            "endOfTurn": {"kind": "smart"},
            "patience": "normal",
            "listening_bar": {"quiet": 0.5, "playing": 0.65},
            "barge_in_min_speech": 0.6,
            "barge_in_window": 2.5,
            "vad_confidence": 0.6,
            "speech_start_ms": 400,
            "pre_roll_ms": 1000,
        },
        "output": {
            "tts": {"catalog": "local", "model": "kokoro-82m-v1.0"},
            "voice": null, "speed": 1, "fade_ms": 30, "language": null,
        },
        "audio_grace_ms": 1000,
        "idle_unload_minutes": 10,
    })
}

#[test]
fn the_webs_settings_with_every_turn_number_are_read() {
    let web: VoiceSettings = serde_json::from_value(web_defaults()).unwrap();
    let input = web.input.as_ref().unwrap();
    assert_eq!(input.listening_bar, Some(ListeningBar { quiet: 0.5, playing: 0.65 }));
    assert_eq!(
        (input.barge_in_min_speech, input.barge_in_window, input.vad_confidence),
        (Some(0.6), Some(2.5), Some(0.6))
    );
    assert_eq!((input.speech_start_ms, input.pre_roll_ms), (Some(400), Some(1000)));
    assert_eq!(web.output.as_ref().unwrap().fade_ms, Some(30.0));
    assert_eq!(web.audio_grace_ms, Some(1000));

    // Still strict: a key the seam does not have is refused, in the listening bar too.
    let mut unknown = web_defaults();
    unknown["input"]["listening_bar"]["loud"] = json!(1);
    assert!(serde_json::from_value::<VoiceSettings>(unknown).is_err());
    // The fade is checked here (the call never sees it), as the seam's web host does.
    for fade in [json!(-1), json!(1001), json!("30")] {
        let mut out_of_range = web_defaults();
        out_of_range["output"]["fade_ms"] = fade;
        assert!(serde_json::from_value::<VoiceSettings>(out_of_range).is_err());
    }
}

#[test]
fn the_turn_numbers_reach_the_native_voice_config_as_they_are() {
    let web: VoiceSettings = serde_json::from_value(web_defaults()).unwrap();
    let choice = choose(&web, &catalogs(true)).unwrap();
    let config = &input(&choice).config;
    assert_eq!(config["listening_bar"], json!({"quiet": 0.5f32, "playing": 0.65f32}));
    assert_eq!(config["barge_in_min_speech"], json!(0.6f32));
    assert_eq!(config["barge_in_window"], json!(2.5f32));
    assert_eq!(config["vad_confidence"], json!(0.6f32));
    assert_eq!((config["speech_start_ms"].clone(), config["pre_roll_ms"].clone()), (json!(400), json!(1000)));
    assert_eq!(choice.config, json!({"audio_grace_ms": 1000, "idle_unload_minutes": 10}));
    // The fade is not the native speaker's to take: the output's configuration has no such key.
    assert!(output(&choice).config.get("fade_ms").is_none());

    // What is absent stays absent, for the call's own default.
    let bare = settings(("local", "whisper-small"), ("local", "kokoro-82m-v1.0"));
    let choice = choose(&bare, &catalogs(true)).unwrap();
    for key in ["listening_bar", "barge_in_window", "vad_confidence", "speech_start_ms", "pre_roll_ms"] {
        assert!(input(&choice).config.get(key).is_none(), "{key}");
    }
    assert!(choice.config.get("audio_grace_ms").is_none());
}

#[test]
fn a_change_carries_the_grace_and_keeps_it_when_not_named() {
    let web: VoiceSettings = serde_json::from_value(web_defaults()).unwrap();
    let change = |value: Value| serde_json::from_value::<SettingsChange>(value).unwrap();
    let longer = web.changed(change(json!({"audio_grace_ms": 2000})));
    assert_eq!(longer.audio_grace_ms, Some(2000));
    assert_eq!(choose(&longer, &catalogs(true)).unwrap().config["audio_grace_ms"], json!(2000));
    assert_eq!(longer.changed(change(json!({"output": null}))).audio_grace_ms, Some(2000));
    // A half the web sends whole, with its turn numbers.
    let input = change(json!({"input": web_defaults()["input"].clone()}));
    assert_eq!(web.changed(input).input, web.input);
}
