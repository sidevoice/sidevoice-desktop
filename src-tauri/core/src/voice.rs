//! The voice call's models and configuration, from the person's choices (`VoiceSettings`, what the page sends through
//! `host.voice.setSettings`) and the engine's catalogue as it ranks its builds here (`Candidate`): the app picks the
//! build of each stage, the voice activity detector, the end-of-turn model and every number the person does not
//! choose.
//!
//! Which build: among those that run here (never Core ML, which the app does not offer, nor MLX, a stub), a
//! speech-to-text model runs on whisper.cpp where it has a build for it (Metal on Apple silicon); anything else on the
//! build the engine recommends, else the first that runs here. The result names the model and build of each slot
//! (sidevoice-voice names none) and carries sidevoice-voice's `VoiceConfig` as JSON.

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
    /// How long the models stay in memory with the call stopped, in minutes; absent for the call's default (10).
    #[serde(default)]
    pub idle_unload_minutes: Option<u32>,
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

/// A model of the catalogue and the build of it the call runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelChoice {
    pub model: String,
    pub build: String,
}

/// What the app makes of the person's choices: the model that fills each of the call's slots, and sidevoice-voice's
/// `VoiceConfig` (which names no model) as JSON.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VoiceChoice {
    pub vad: ModelChoice,
    pub stt: ModelChoice,
    pub tts: ModelChoice,
    /// For `smart-turn`: the first model of the catalogue that ends turns and runs here.
    pub end_of_turn: Option<ModelChoice>,
    pub config: Value,
}

impl VoiceChoice {
    /// Whether `other` fills the call's slots with the same models (its configuration aside).
    pub fn same_models(&self, other: &VoiceChoice) -> bool {
        (&self.vad, &self.stt, &self.tts, &self.end_of_turn) == (&other.vad, &other.stt, &other.tts, &other.end_of_turn)
    }
}

/// The models and the configuration for `settings`, on the builds of `catalogue` that run here.
pub fn choose(settings: &VoiceSettings, catalogue: &[Candidate]) -> Result<VoiceChoice, Refusal> {
    let pick = |model: &str, task: &str, named: Option<&str>| {
        build(catalogue, model, task, named).map(|build| ModelChoice { model: model.to_string(), build })
    };
    let vad = pick(VAD_MODEL, "vad", None)?;
    let stt = pick(&settings.stt.model, "stt", settings.stt.build.as_deref())?;
    let tts = pick(&settings.tts.model, "tts", settings.tts.build.as_deref())?;
    let end_of_turn = settings.end_of_turn.unwrap_or(EndOfTurn::Silence);
    let ends_turns = match end_of_turn {
        EndOfTurn::Silence => None,
        EndOfTurn::SmartTurn => Some(
            catalogue
                .iter()
                .filter(|c| c.capabilities.iter().any(|capability| capability == END_OF_TURN))
                .find_map(|c| pick(&c.id, END_OF_TURN, None).ok())
                .ok_or_else(|| Refusal {
                    key: "voice_end_of_turn_unavailable",
                    model: String::new(),
                    message: "No model of the engine's catalogue that ends turns runs on this device.".into(),
                })?,
        ),
    };
    let mut config = json!({
        "language": settings.stt.language,
        "voice": settings.tts.voice,
        "speed": settings.tts.speed.unwrap_or(1.0),
        "end_of_turn": end_of_turn,
    });
    if let Some(patience) = settings.patience {
        config["patience"] = json!(patience);
    }
    if let Some(minutes) = settings.idle_unload_minutes {
        config["idle_unload_minutes"] = json!(minutes);
    }
    Ok(VoiceChoice { vad, stt, tts, end_of_turn: ends_turns, config })
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
