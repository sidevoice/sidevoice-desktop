use super::*;

fn build(id: &str, backend: &str, accelerator: Option<&str>) -> CandidateBuild {
    CandidateBuild {
        id: id.into(),
        backend: backend.into(),
        accelerator: accelerator.map(Into::into),
        available: accelerator.is_some(),
    }
}

fn catalogue(apple_silicon: bool) -> Vec<Candidate> {
    let metal = apple_silicon.then_some("metal").or(Some("cpu"));
    vec![
        Candidate {
            id: "silero-vad".into(),
            capabilities: vec!["vad".into()],
            builds: vec![
                build("silero-vad/sherpa-onnx-fp32", "sherpa-onnx", Some("cpu")),
                build("silero-vad/transformers-js-fp32", "transformers-js", None),
            ],
            recommended: Some("silero-vad/sherpa-onnx-fp32".into()),
        },
        Candidate {
            id: "whisper-small".into(),
            capabilities: vec!["stt".into()],
            builds: vec![
                build("whisper-small/mlx-fp16", "mlx", apple_silicon.then_some("metal")),
                build("whisper-small/sherpa-onnx-int8", "sherpa-onnx", Some("cpu")),
                build("whisper-small/whisper-cpp-q5_1", "whisper-cpp", metal),
            ],
            recommended: Some("whisper-small/sherpa-onnx-int8".into()),
        },
        Candidate {
            id: "kokoro-82m-v1.0".into(),
            capabilities: vec!["tts".into()],
            builds: vec![build("kokoro-82m-v1.0/sherpa-onnx-int8", "sherpa-onnx", Some("cpu"))],
            recommended: Some("kokoro-82m-v1.0/sherpa-onnx-int8".into()),
        },
        Candidate {
            id: "gpt-4o-mini-transcribe".into(),
            capabilities: vec!["stt".into()],
            builds: vec![build("gpt-4o-mini-transcribe/openai", "openai", Some("remote"))],
            recommended: Some("gpt-4o-mini-transcribe/openai".into()),
        },
        Candidate {
            id: "parakeet".into(),
            capabilities: vec!["stt".into()],
            builds: vec![build("parakeet/coreml", "sherpa-onnx", Some("coreml"))],
            recommended: Some("parakeet/coreml".into()),
        },
    ]
}

fn settings(stt: &str, tts: &str) -> VoiceSettings {
    serde_json::from_value(json!({
        "stt": {"model": stt, "language": "es"},
        "tts": {"model": tts, "voice": "ef_dora"},
        "patience": "calm",
        "idle_unload_minutes": 30,
    }))
    .unwrap()
}

#[test]
fn whisper_runs_on_whisper_cpp_and_the_rest_on_the_recommended_build() {
    for apple_silicon in [true, false] {
        let config = config(&settings("whisper-small", "kokoro-82m-v1.0"), &catalogue(apple_silicon)).unwrap();
        assert_eq!(
            config,
            json!({
                "vad": {"model": "silero-vad", "build": "silero-vad/sherpa-onnx-fp32"},
                "stt": {"model": "whisper-small", "build": "whisper-small/whisper-cpp-q5_1", "language": "es"},
                "tts": {"model": "kokoro-82m-v1.0", "build": "kokoro-82m-v1.0/sherpa-onnx-int8", "voice": "ef_dora",
                        "speed": 1.0},
                "end_of_turn": "silence",
                "patience": "calm",
                "idle_unload_minutes": 30,
            })
        );
    }
}

#[test]
fn a_remote_model_runs_on_its_provider() {
    let config = config(&settings("gpt-4o-mini-transcribe", "kokoro-82m-v1.0"), &catalogue(true)).unwrap();
    assert_eq!(config["stt"]["build"], "gpt-4o-mini-transcribe/openai");
}

#[test]
fn what_cannot_run_is_refused_by_key() {
    let key = |stt: &str, tts: &str| config(&settings(stt, tts), &catalogue(true)).unwrap_err().key;
    assert_eq!(key("whisper-nope", "kokoro-82m-v1.0"), "voice_model_unknown");
    assert_eq!(key("kokoro-82m-v1.0", "kokoro-82m-v1.0"), "voice_model_wrong_task");
    assert_eq!(key("parakeet", "kokoro-82m-v1.0"), "voice_model_unfit", "never Core ML");
    let without_vad: Vec<Candidate> = catalogue(true).into_iter().filter(|c| c.id != VAD_MODEL).collect();
    let refused = config(&settings("whisper-small", "kokoro-82m-v1.0"), &without_vad).unwrap_err();
    assert_eq!((refused.key, refused.model.as_str()), ("voice_model_unknown", "silero-vad"));
}

#[test]
fn settings_are_read_strictly_and_their_choices_are_optional() {
    let minimal: VoiceSettings =
        serde_json::from_value(json!({"stt": {"model": "whisper-small"}, "tts": {"model": "kokoro-82m-v1.0"}}))
            .unwrap();
    let config = config(&minimal, &catalogue(true)).unwrap();
    assert_eq!(config["stt"]["language"], Value::Null);
    assert_eq!(config["tts"]["voice"], Value::Null);
    assert!(config.get("patience").is_none());
    let unknown = json!({"stt": {"model": "a", "prompt": "x"}, "tts": {"model": "b"}});
    assert!(serde_json::from_value::<VoiceSettings>(unknown).is_err());
}

#[test]
fn a_named_build_runs_if_it_runs_here_and_the_app_offers_it() {
    let named = |build: &str| {
        let mut chosen = settings("whisper-small", "kokoro-82m-v1.0");
        chosen.stt.build = Some(build.into());
        config(&chosen, &catalogue(true))
    };
    assert_eq!(named("whisper-small/sherpa-onnx-int8").unwrap()["stt"]["build"], "whisper-small/sherpa-onnx-int8");
    for unfit in ["whisper-small/mlx-fp16", "whisper-small/nope", "kokoro-82m-v1.0/sherpa-onnx-int8"] {
        assert_eq!(named(unfit).unwrap_err().key, "voice_build_unfit", "{unfit}");
    }
}

#[test]
fn smart_turn_needs_a_model_that_ends_turns() {
    let mut chosen = settings("whisper-small", "kokoro-82m-v1.0");
    chosen.end_of_turn = Some(EndOfTurn::SmartTurn);
    assert_eq!(config(&chosen, &catalogue(true)).unwrap_err().key, "voice_end_of_turn_unavailable");
    let mut with_turns = catalogue(true);
    with_turns.push(Candidate {
        id: "smart-turn-v3".into(),
        capabilities: vec!["end-of-turn".into()],
        builds: vec![build("smart-turn-v3/sherpa-onnx", "sherpa-onnx", Some("cpu"))],
        recommended: None,
    });
    assert_eq!(config(&chosen, &with_turns).unwrap()["end_of_turn"], "smart-turn");
}
