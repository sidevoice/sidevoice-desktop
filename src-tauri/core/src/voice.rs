//! The voice call's models and configuration, from the person's choices (`VoiceSettings`, what the page sends through
//! `host.voice.setSettings`) and the engine's catalogues as they list their models (`CatalogCandidates`): each slot is
//! a model of a catalogue (`{catalog, model}`), which picks the build itself when it loads it; the app picks the voice
//! activity detector and the end-of-turn model, from the local catalogue, and every number the person does not choose.
//!
//! The result names the catalogue and model of each slot (sidevoice-voice names none) and carries sidevoice-voice's
//! `VoiceConfig` as JSON. What cannot be made is refused with a stable code the page translates.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[cfg(test)]
mod tests;

/// The id of the catalogue of models that run on this device.
pub const LOCAL_CATALOG: &str = "local";
/// The family of the voice activity detector a call prefers.
const VAD_FAMILY: &str = "silero-vad";

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

/// Speech to text: a model of a catalogue, and the language spoken.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SttChoice {
    pub catalog: String,
    pub model: String,
    /// A BCP 47 tag; absent or null to detect it.
    #[serde(default)]
    pub language: Option<String>,
}

/// Text to speech: a model of a catalogue, its voice and speed.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TtsChoice {
    pub catalog: String,
    pub model: String,
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

/// A catalogue of the engine, as far as choosing from it needs: why it is not current, and the models it lists.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogCandidates {
    pub id: String,
    /// Why it lists nothing, or not the latest (`credential-missing`, ...): the engine's code.
    pub reason: Option<String>,
    /// What a provider said when it refused.
    pub detail: Option<String>,
    pub models: Vec<Candidate>,
}

/// A model of a catalogue, as far as choosing it needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    /// A local model's family; `None` for a provider's.
    pub family: Option<String>,
    /// `stt`, `tts`, `vad`, `end-of-turn`.
    pub capabilities: Vec<String>,
    /// Whether it runs here: a local model with a build that runs; a provider's always.
    pub runs: bool,
}

/// Why a configuration cannot be made: the page's code for it (`model-unknown`, `model-unfit`, `catalog-not-found`,
/// `vad-unavailable`, `end-of-turn-unavailable`, or the engine's own), with what a provider said.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Refusal {
    pub code: String,
    pub model: String,
    pub detail: Option<String>,
    pub message: String,
}

/// A slot's model: its catalogue and its id there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelChoice {
    pub catalog: String,
    pub model: String,
}

/// What the app makes of the person's choices: the model that fills each of the call's slots, and sidevoice-voice's
/// `VoiceConfig` (which names no model) as JSON.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VoiceChoice {
    pub vad: ModelChoice,
    pub stt: ModelChoice,
    pub tts: ModelChoice,
    /// For `smart-turn`: the local catalogue's first model that ends turns and runs here.
    pub end_of_turn: Option<ModelChoice>,
    pub config: Value,
}

impl VoiceChoice {
    /// Whether `other` fills the call's slots with the same models (its configuration aside): other models come when a
    /// slot's catalogue or model changes, or the end of turn does.
    pub fn same_models(&self, other: &VoiceChoice) -> bool {
        (&self.vad, &self.stt, &self.tts, &self.end_of_turn) == (&other.vad, &other.stt, &other.tts, &other.end_of_turn)
    }
}

fn refusal(code: &str, model: &str, message: String) -> Refusal {
    Refusal { code: code.to_string(), model: model.to_string(), detail: None, message }
}

/// The models and the configuration for `settings`, from what `catalogs` list.
pub fn choose(settings: &VoiceSettings, catalogs: &[CatalogCandidates]) -> Result<VoiceChoice, Refusal> {
    let stt = slot(catalogs, &settings.stt.catalog, &settings.stt.model, "stt")?;
    let tts = slot(catalogs, &settings.tts.catalog, &settings.tts.model, "tts")?;
    let local: &[Candidate] = catalogs.iter().find(|c| c.id == LOCAL_CATALOG).map_or(&[], |c| &c.models);
    fn runs_as<'a>(local: &'a [Candidate], task: &'a str) -> impl Iterator<Item = &'a Candidate> {
        local.iter().filter(move |m| m.runs && m.capabilities.iter().any(|capability| capability == task))
    }
    let vad = runs_as(local, "vad")
        .find(|m| m.family.as_deref() == Some(VAD_FAMILY))
        .or_else(|| runs_as(local, "vad").next())
        .map(|m| ModelChoice { catalog: LOCAL_CATALOG.into(), model: m.id.clone() })
        .ok_or_else(|| refusal("vad-unavailable", "", "No voice activity detector runs on this device.".into()))?;
    let end_of_turn = settings.end_of_turn.unwrap_or(EndOfTurn::Silence);
    let ends_turns = match end_of_turn {
        EndOfTurn::Silence => None,
        EndOfTurn::SmartTurn => Some(
            runs_as(local, "end-of-turn")
                .next()
                .map(|m| ModelChoice { catalog: LOCAL_CATALOG.into(), model: m.id.clone() })
                .ok_or_else(|| {
                    refusal("end-of-turn-unavailable", "", "No model that ends turns runs on this device.".into())
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

/// Model `model` of catalogue `catalog`, for `task`: one the catalogue lists for it and that runs here. A catalogue
/// that lists nothing for a reason (a provider with no key, ...) refuses with that reason and what the provider said.
fn slot(catalogs: &[CatalogCandidates], catalog: &str, model: &str, task: &str) -> Result<ModelChoice, Refusal> {
    let listed = catalogs
        .iter()
        .find(|c| c.id == catalog)
        .ok_or_else(|| refusal("catalog-not-found", model, format!("There is no catalogue {catalog}.")))?;
    let Some(candidate) =
        listed.models.iter().find(|m| m.id == model && m.capabilities.iter().any(|capability| capability == task))
    else {
        return Err(match &listed.reason {
            Some(reason) => Refusal {
                code: reason.clone(),
                model: model.to_string(),
                detail: listed.detail.clone(),
                message: format!("{catalog} lists no models now ({reason})."),
            },
            None => refusal("model-unknown", model, format!("{catalog} lists no {task} model {model}.")),
        });
    };
    if !candidate.runs {
        return Err(refusal("model-unfit", model, format!("{model} has no build that runs on this device.")));
    }
    Ok(ModelChoice { catalog: catalog.to_string(), model: model.to_string() })
}
