//! The voice call's configuration, from the person's choices (`VoiceSettings`, what the page sends through
//! `host.voice.setSettings`) and the engine's catalogue as it ranks its builds here (`Candidate`): the app picks the
//! build of each stage, the voice activity detector and every number the person does not choose.
//!
//! Which build: among those that run here (never Core ML, which the app does not offer, nor MLX, a stub), a
//! speech-to-text model runs on whisper.cpp where it has a build for it (Metal on Apple silicon); anything else on the
//! build the engine recommends, else the first that runs here. The result is sidevoice-voice's `VoiceConfig` as JSON.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[cfg(test)]
mod tests;

/// The voice activity detector every call runs: Silero, on sherpa-onnx.
pub const VAD_MODEL: &str = "silero-vad";
/// The capability of a model that ends turns (sidevoice-engine#69).
const END_OF_TURN: &str = "end-of-turn";
/// The backend speech to text prefers where a model has a build for it.
const PREFERRED_STT_BACKEND: &str = "whisper-cpp";

/// What the person chooses, as the page sends it. Read strictly: an unknown key is refused.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceSettings {
    pub stt: SttChoice,
    pub tts: TtsChoice,
    #[serde(default)]
    pub patience: Option<Patience>,
    #[serde(default)]
    pub end_of_turn: Option<EndOfTurn>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SttChoice {
    pub model: String,
    /// One of the model's builds that runs here; absent or null for the app's choice.
    #[serde(default)]
    pub build: Option<String>,
    /// A BCP 47 tag; absent or null to detect it.
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TtsChoice {
    pub model: String,
    /// One of the model's builds that runs here; absent or null for the app's choice.
    #[serde(default)]
    pub build: Option<String>,
    /// One of the model's voices; absent or null for its first.
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub speed: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Patience {
    Fast,
    Normal,
    Calm,
}

/// What ends a turn: a pause, or the engine's `end-of-turn` model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EndOfTurn {
    Silence,
    SmartTurn,
}

/// A model of the engine's catalogue, as far as choosing its build needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    /// `stt`, `tts`, `vad`.
    pub capabilities: Vec<String>,
    pub builds: Vec<CandidateBuild>,
    pub recommended: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CandidateBuild {
    pub id: String,
    pub backend: String,
    /// The accelerator it would run on here (`cpu`, `metal`, `coreml`, `remote`…), when it runs here.
    pub accelerator: Option<String>,
    pub available: bool,
}

/// Why a configuration cannot be made: a stable key and its parameters, as the bridge's refusals.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Refusal {
    pub key: &'static str,
    pub model: String,
    pub message: String,
}

/// sidevoice-voice's `VoiceConfig` for `settings`, on the builds of `catalogue` that run here.
pub fn config(settings: &VoiceSettings, catalogue: &[Candidate]) -> Result<Value, Refusal> {
    let vad = build(catalogue, VAD_MODEL, "vad", None)?;
    let stt = build(catalogue, &settings.stt.model, "stt", settings.stt.build.as_deref())?;
    let tts = build(catalogue, &settings.tts.model, "tts", settings.tts.build.as_deref())?;
    let end_of_turn = settings.end_of_turn.unwrap_or(EndOfTurn::Silence);
    let can_end_turns = catalogue.iter().any(|c| c.capabilities.iter().any(|capability| capability == END_OF_TURN));
    if end_of_turn == EndOfTurn::SmartTurn && !can_end_turns {
        return Err(Refusal {
            key: "voice_end_of_turn_unavailable",
            model: String::new(),
            message: "No model of the engine's catalogue ends turns.".into(),
        });
    }
    let mut config = json!({
        "vad": { "model": VAD_MODEL, "build": vad },
        "stt": { "model": settings.stt.model, "build": stt, "language": settings.stt.language },
        "tts": {
            "model": settings.tts.model, "build": tts, "voice": settings.tts.voice,
            "speed": settings.tts.speed.unwrap_or(1.0),
        },
        "end_of_turn": end_of_turn,
    });
    if let Some(patience) = settings.patience {
        config["patience"] = json!(patience);
    }
    Ok(config)
}

/// The build of `model` that the call runs, for `task`: `named` when the person named one, which must be one that runs
/// here and the app offers.
fn build(catalogue: &[Candidate], model: &str, task: &str, named: Option<&str>) -> Result<String, Refusal> {
    let refuse = |key, message: String| Refusal { key, model: model.to_string(), message };
    let candidate = catalogue
        .iter()
        .find(|candidate| candidate.id == model)
        .ok_or_else(|| refuse("voice_model_unknown", format!("{model} is not in the engine's catalogue.")))?;
    if !candidate.capabilities.iter().any(|capability| capability == task) {
        return Err(refuse("voice_model_wrong_task", format!("{model} cannot do {task}.")));
    }
    let offered: Vec<&CandidateBuild> = candidate
        .builds
        .iter()
        .filter(|b| b.available && b.backend != "mlx" && b.accelerator.as_deref() != Some("coreml"))
        .collect();
    if let Some(named) = named {
        return offered.iter().find(|b| b.id == named).map(|b| b.id.clone()).ok_or_else(|| {
            refuse("voice_build_unfit", format!("{named} is not a build of {model} that runs on this device."))
        });
    }
    let preferred = (task == "stt").then(|| offered.iter().find(|b| b.backend == PREFERRED_STT_BACKEND)).flatten();
    let recommended = offered.iter().find(|b| Some(&b.id) == candidate.recommended.as_ref());
    preferred
        .or(recommended)
        .or(offered.first())
        .map(|b| b.id.clone())
        .ok_or_else(|| refuse("voice_model_unfit", format!("{model} has no build that runs on this device.")))
}
