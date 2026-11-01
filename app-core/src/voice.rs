//! The voice call's models and configuration, from the person's choices (`VoiceSettings`, what the page sends through
//! `host.voice.start` and `preload`, and `SettingsChange`, what it sends through a call's `update`) and the engine's
//! catalogues as they list their models (`CatalogCandidates`). The settings have the voice seam's shape
//! (sidevoice-voice `js/voice-host.d.ts`): an input half and an output half, each optional, at least one. Each slot a
//! half uses is a model of a catalogue (`{catalog, model}`), loaded with the build the person chose for a local one
//! (`build`), else with the build its catalogue picks; the app picks the voice activity detector (only for an input
//! half) and, for `smart` end of turn, the end-of-turn model, from the local catalogue, and every number the person
//! does not choose. A half that is absent has no slot and loads nothing.
//!
//! The result names the catalogue and model of each slot (sidevoice-voice names none) and carries sidevoice-voice's
//! configuration of each half as JSON. What cannot be made is refused with a stable code the page translates. A take
//! the call hands over reaches the page as `take` makes it.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};

pub mod host;
pub mod meters;
pub mod models;
pub mod take;
#[cfg(test)]
mod tests;

/// The id of the catalogue of models that run on this device.
pub const LOCAL_CATALOG: &str = "local";
/// The family of the voice activity detector a call prefers.
const VAD_FAMILY: &str = "silero-vad";

/// What the person chooses for a call, as the page sends it: the seam's `VoiceSettings`. Read strictly: an unknown key
/// is refused.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceSettings {
    /// Listening: absent (or null), no microphone, no detector, no transcriber, no end-of-turn model.
    #[serde(default)]
    pub input: Option<InputSettings>,
    /// Speaking: absent (or null), no speaker model and no playback.
    #[serde(default)]
    pub output: Option<OutputSettings>,
    /// How long after a turn ends what is to be said waits, in milliseconds; absent for the call's default. Its range
    /// is checked by the call (sidevoice-voice's `VoiceConfig`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_grace_ms: Option<u32>,
    /// How long the models stay in memory with the call stopped, in minutes; absent for the call's default (10).
    #[serde(default)]
    pub idle_unload_minutes: Option<u32>,
}

/// The input half: the transcriber, what ends a turn, and how it listens. The turn's numbers are the web's to choose;
/// each crosses to the call as it is, which checks its range and takes its own default for one absent
/// (sidevoice-voice's `InputConfig`).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InputSettings {
    pub stt: SttChoice,
    /// `{kind: "silence"}` when absent.
    #[serde(default, rename = "endOfTurn")]
    pub end_of_turn: Option<EndOfTurn>,
    #[serde(default)]
    pub patience: Option<Patience>,
    /// How loud speech must be to count, while nothing plays and while something does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listening_bar: Option<ListeningBar>,
    /// Qualified speech required to cut sounding agent audio, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barge_in_min_speech: Option<f32>,
    /// The sliding window that speech falls in, as a multiple of `barge_in_min_speech`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barge_in_window: Option<f32>,
    /// The detector's confidence from which a window is speech.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vad_confidence: Option<f32>,
    /// How long speech must go on before it starts a turn in silence, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech_start_ms: Option<u32>,
    /// The audio kept before a turn opens, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_roll_ms: Option<u32>,
}

/// How loud speech must be to count, as a volume from 0 to 1: while nothing plays (`quiet`) and while something does
/// (`playing`, what a barge-in must clear).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ListeningBar {
    pub quiet: f32,
    pub playing: f32,
}

/// The output half: the speaker model, its voice and speed.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutputSettings {
    pub tts: TtsChoice,
    /// One of the model's voices; absent or null for its first.
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub speed: Option<f32>,
    /// How long the speaker fades out when playback is cut, in milliseconds, 0 to 1000: read and checked as the seam's
    /// web host does, but not applied. The native speaker's fade is fixed (sidevoice-voice's `io::native::FADE_MS`,
    /// 30 ms, the web's default) and no configuration reaches it.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "fade_ms")]
    pub fade_ms: Option<f64>,
    /// The language of what is said when `say` gives none; absent: the input's.
    #[serde(default)]
    pub language: Option<String>,
}

/// A fade within what the seam takes (`output.fade_ms`, 0 to 1000 ms), or none.
fn fade_ms<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<f64>, D::Error> {
    match Option::<f64>::deserialize(deserializer)? {
        Some(ms) if !(ms.is_finite() && (0.0..=1000.0).contains(&ms)) => {
            Err(serde::de::Error::custom("output.fade_ms must be a number between 0 and 1000"))
        }
        fade => Ok(fade),
    }
}

/// Speech to text: a model of a catalogue, and the language spoken.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SttChoice {
    pub catalog: String,
    pub model: String,
    /// A BCP 47 tag; absent or null to detect it. Not part of the model's choice.
    #[serde(default)]
    pub language: Option<String>,
    /// For a local model, the build of it to load; absent for the one its catalogue picks. Ignored for a provider's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
}

/// Text to speech: a model of a catalogue.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TtsChoice {
    pub catalog: String,
    pub model: String,
    /// For a local model, the build of it to load; absent for the one its catalogue picks. Ignored for a provider's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Patience {
    Fast,
    Normal,
    Calm,
}

/// What ends a turn: a pause (`ms` after the detector says speech stopped; the patience's when absent), which loads no
/// end-of-turn model, or the engine's end-of-turn model, asked at each pause.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum EndOfTurn {
    Silence {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ms: Option<u32>,
    },
    Smart {
        /// The local model that ends turns, when it runs here; else (or absent, or null) the first that does.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<ModelRef>,
    },
}

/// A model as the page names it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRef {
    pub catalog: String,
    pub model: String,
}

/// A change of a running call's settings, as the page sends it: a half that is absent stays as it is, `null` removes
/// it, and settings replace it (or add it). Read strictly, like the settings.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsChange {
    #[serde(default, deserialize_with = "present")]
    pub input: Option<Option<InputSettings>>,
    #[serde(default, deserialize_with = "present")]
    pub output: Option<Option<OutputSettings>>,
    #[serde(default)]
    pub audio_grace_ms: Option<u32>,
    #[serde(default)]
    pub idle_unload_minutes: Option<u32>,
}

/// A field that is there, `null` included.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

impl VoiceSettings {
    /// The settings after `change`.
    pub fn changed(&self, change: SettingsChange) -> VoiceSettings {
        VoiceSettings {
            input: change.input.unwrap_or_else(|| self.input.clone()),
            output: change.output.unwrap_or_else(|| self.output.clone()),
            audio_grace_ms: change.audio_grace_ms.or(self.audio_grace_ms),
            idle_unload_minutes: change.idle_unload_minutes.or(self.idle_unload_minutes),
        }
    }
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
    /// A local model's builds; none for a provider's.
    pub builds: Vec<BuildCandidate>,
}

/// A build of a local model, as far as choosing it needs.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildCandidate {
    pub id: String,
    /// Why it does not run here, by the engine's code (`memory`, `backend-not-in-this-build`, ...); `None` when it runs.
    pub unfit: Option<String>,
}

/// Why a configuration cannot be made: the page's code for it (`halves-missing`, `model-unknown`, `model-unfit`,
/// `catalog-not-found`, `vad-unavailable`, `end-of-turn-unavailable`, or the engine's own: `build-not-found`, why a
/// chosen build does not run here, a provider's status), with what a provider said.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Refusal {
    pub code: String,
    pub model: String,
    pub detail: Option<String>,
    pub message: String,
}

/// A slot's model: its catalogue, its id there, and for a local one the build chosen (`None`: the catalogue picks).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelChoice {
    pub catalog: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
}

impl ModelChoice {
    /// The local catalogue's `model`, with the build it picks.
    pub fn local(model: &str) -> Self {
        Self { catalog: LOCAL_CATALOG.into(), model: model.into(), build: None }
    }
}

/// What the app makes of the person's choices: for each half the call has, the model that fills each of its slots and
/// its configuration, and what holds for the whole call. The configurations are sidevoice-voice's (`InputConfig`,
/// `OutputConfig`, `VoiceConfig`, which name no model) as JSON.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VoiceChoice {
    pub input: Option<InputChoice>,
    pub output: Option<OutputChoice>,
    /// `VoiceConfig`.
    pub config: Value,
}

/// The input half's slots: the detector and the transcriber, and for `smart` the model that ends turns.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InputChoice {
    pub vad: ModelChoice,
    pub stt: ModelChoice,
    pub end_of_turn: Option<ModelChoice>,
    /// `InputConfig`.
    pub config: Value,
}

/// The output half's slot: the speaker.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutputChoice {
    pub tts: ModelChoice,
    /// `OutputConfig`.
    pub config: Value,
}

impl InputChoice {
    /// Whether `other` fills the half's slots with the same models (its configuration aside): a half with the same
    /// models keeps what it loaded when the call is updated.
    pub fn same_models(&self, other: &InputChoice) -> bool {
        (&self.vad, &self.stt, &self.end_of_turn) == (&other.vad, &other.stt, &other.end_of_turn)
    }
}

impl OutputChoice {
    /// Whether `other` fills the half's slot with the same model (its configuration aside).
    pub fn same_models(&self, other: &OutputChoice) -> bool {
        self.tts == other.tts
    }
}

fn refusal(code: &str, model: &str, message: String) -> Refusal {
    Refusal { code: code.to_string(), model: model.to_string(), detail: None, message }
}

/// The models and the configuration for `settings`, from what `catalogs` list. Only the halves `settings` has are
/// looked at.
pub fn choose(settings: &VoiceSettings, catalogs: &[CatalogCandidates]) -> Result<VoiceChoice, Refusal> {
    if settings.input.is_none() && settings.output.is_none() {
        return Err(refusal("halves-missing", "", "A call needs an input half, an output half or both.".into()));
    }
    let local: &[Candidate] = catalogs.iter().find(|c| c.id == LOCAL_CATALOG).map_or(&[], |c| &c.models);
    let input = settings.input.as_ref().map(|input| choose_input(input, catalogs, local)).transpose()?;
    let output = settings.output.as_ref().map(|output| choose_output(output, catalogs)).transpose()?;
    let mut config = json!({});
    if let Some(ms) = settings.audio_grace_ms {
        config["audio_grace_ms"] = json!(ms);
    }
    if let Some(minutes) = settings.idle_unload_minutes {
        config["idle_unload_minutes"] = json!(minutes);
    }
    Ok(VoiceChoice { input, output, config })
}

/// The local models that run here and have `task`.
fn runs_as<'a>(local: &'a [Candidate], task: &'a str) -> impl Iterator<Item = &'a Candidate> {
    local.iter().filter(move |m| m.runs && m.capabilities.iter().any(|capability| capability == task))
}

fn choose_input(
    input: &InputSettings,
    catalogs: &[CatalogCandidates],
    local: &[Candidate],
) -> Result<InputChoice, Refusal> {
    let stt = slot(catalogs, &input.stt.catalog, &input.stt.model, input.stt.build.as_deref(), "stt")?;
    let vad = runs_as(local, "vad")
        .find(|m| m.family.as_deref() == Some(VAD_FAMILY))
        .or_else(|| runs_as(local, "vad").next())
        .map(|m| ModelChoice::local(&m.id))
        .ok_or_else(|| refusal("vad-unavailable", "", "No voice activity detector runs on this device.".into()))?;
    let (end_of_turn, ends_turns) = match input.end_of_turn.clone().unwrap_or(EndOfTurn::Silence { ms: None }) {
        silence @ EndOfTurn::Silence { .. } => (silence, None),
        // As the web's voice source: a chosen model that is not one that runs here gives way to the first that does.
        EndOfTurn::Smart { model } => {
            let wanted = model.filter(|wanted| wanted.catalog == LOCAL_CATALOG);
            let chosen = runs_as(local, "end-of-turn")
                .find(|m| wanted.as_ref().is_some_and(|wanted| wanted.model == m.id))
                .or_else(|| runs_as(local, "end-of-turn").next())
                .map(|m| ModelChoice::local(&m.id))
                .ok_or_else(|| {
                    refusal("end-of-turn-unavailable", "", "No model that ends turns runs on this device.".into())
                })?;
            (EndOfTurn::Smart { model: None }, Some(chosen))
        }
    };
    let mut config = json!({ "language": input.stt.language, "end_of_turn": end_of_turn });
    if let Some(patience) = input.patience {
        config["patience"] = json!(patience);
    }
    if let Some(bar) = input.listening_bar {
        config["listening_bar"] = json!(bar);
    }
    if let Some(seconds) = input.barge_in_min_speech {
        config["barge_in_min_speech"] = json!(seconds);
    }
    if let Some(window) = input.barge_in_window {
        config["barge_in_window"] = json!(window);
    }
    if let Some(confidence) = input.vad_confidence {
        config["vad_confidence"] = json!(confidence);
    }
    if let Some(ms) = input.speech_start_ms {
        config["speech_start_ms"] = json!(ms);
    }
    if let Some(ms) = input.pre_roll_ms {
        config["pre_roll_ms"] = json!(ms);
    }
    Ok(InputChoice { vad, stt, end_of_turn: ends_turns, config })
}

fn choose_output(output: &OutputSettings, catalogs: &[CatalogCandidates]) -> Result<OutputChoice, Refusal> {
    let tts = slot(catalogs, &output.tts.catalog, &output.tts.model, output.tts.build.as_deref(), "tts")?;
    let config = json!({
        "language": output.language,
        "voice": output.voice,
        "speed": output.speed.unwrap_or(1.0),
    });
    Ok(OutputChoice { tts, config })
}

/// Model `model` of catalogue `catalog`, for `task`: one the catalogue lists for it and that runs here. A catalogue
/// that lists nothing for a reason (a provider with no key, ...) refuses with that reason and what the provider said.
/// A local model's `build`, when named (an empty one is not), is one of its builds that runs here, checked once the
/// model is (`build-not-found` otherwise, or the engine's code for why it does not run); a provider's models have no
/// builds, and a `build` named for one is ignored. As the web's voice source.
fn slot(
    catalogs: &[CatalogCandidates],
    catalog: &str,
    model: &str,
    build: Option<&str>,
    task: &str,
) -> Result<ModelChoice, Refusal> {
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
    let build = match build.filter(|wanted| !wanted.is_empty()) {
        Some(wanted) if catalog == LOCAL_CATALOG => {
            let found = candidate
                .builds
                .iter()
                .find(|b| b.id == wanted)
                .ok_or_else(|| refusal("build-not-found", model, format!("{model} has no build {wanted}.")))?;
            if let Some(unfit) = &found.unfit {
                return Err(refusal(unfit, model, format!("{model}'s build {wanted} does not run on this device.")));
            }
            Some(wanted.to_string())
        }
        _ => None,
    };
    Ok(ModelChoice { catalog: catalog.to_string(), model: model.to_string(), build })
}

/// A catalogue of the engine, as choosing from it needs: a local model runs here when one of its builds does; a
/// provider's always does. A build that does not run says why by its first reason's code.
pub fn candidates(view: &crate::engine::CatalogView) -> CatalogCandidates {
    CatalogCandidates {
        id: view.id.clone(),
        reason: view.status.reason.as_ref().map(|reason| reason.code.clone()),
        detail: view.status.detail.clone(),
        models: view
            .models
            .iter()
            .map(|model| Candidate {
                id: model.id.clone(),
                family: model.local.as_ref().map(|local| local.family.clone()),
                capabilities: model.capabilities.iter().map(|c| c.to_string()).collect(),
                runs: model.local.as_ref().is_none_or(|local| local.builds.iter().any(|build| build.available)),
                builds: model.local.as_ref().map_or_else(Vec::new, |local| {
                    local
                        .builds
                        .iter()
                        .map(|build| BuildCandidate {
                            id: build.id.clone(),
                            unfit: (!build.available).then(|| {
                                build.reasons.first().map_or("no-build-available".into(), |reason| reason.code.clone())
                            }),
                        })
                        .collect()
                }),
            })
            .collect(),
    }
}
