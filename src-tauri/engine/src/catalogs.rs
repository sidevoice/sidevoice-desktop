//! The engine's catalogues as the page lists them (`host.engine.catalogs()`, docs/BRIDGE.md → "The native engine"):
//! the local catalogue first, then each remote provider's, each with how it stands and every model it lists, in the
//! shape `@sidevoice/engine` gives the page on the web (its JavaScript names: `parametersM`, `recommendedBuild`, ...).

use serde::Serialize;
use sidevoice_engine::{
    Capability, CatalogStatus, Gender, LocalModelInfo, ModelBuild, ModelInfo, Reason, SpeedRange, Voice,
};

use crate::accelerator_name;

/// One catalogue: its id (`local`, or the provider's), its name (`None` for the local one), how it stands, its models.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CatalogView {
    pub id: String,
    pub name: Option<String>,
    pub status: StatusView,
    pub models: Vec<ModelView>,
}

/// `{reason?, stale, detail?}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatusView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<ReasonView>,
    pub stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// `{code, params: {needs?, has?}}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReasonView {
    pub code: String,
    pub params: ReasonParams,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReasonParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has: Option<u32>,
}

/// A model as its catalogue lists it, `{id, capabilities, languages, voices, speed?}`; a local one also has its family,
/// size, licence, builds and what is installed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelView {
    pub id: String,
    pub capabilities: Vec<&'static str>,
    pub languages: Vec<String>,
    pub voices: Vec<VoiceView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<SpeedView>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalView {
    pub family: String,
    pub parameters_m: u32,
    pub license: String,
    pub installed: bool,
    pub builds: Vec<BuildView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommended_build: Option<String>,
}

/// `{id, backend, accelerator?, precision, downloadBytes, memoryMb, available, reasons, installed}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildView {
    pub id: String,
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accelerator: Option<String>,
    pub precision: String,
    pub download_bytes: u64,
    pub memory_mb: u32,
    pub available: bool,
    pub reasons: Vec<ReasonView>,
    pub installed: bool,
}

/// `{id, name?, languages, gender?}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VoiceView {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub languages: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gender: Option<&'static str>,
}

/// `{min, max}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SpeedView {
    pub min: f32,
    pub max: f32,
}

/// A capability by the catalogue's id for it.
pub fn capability_name(capability: Capability) -> &'static str {
    match capability {
        Capability::Stt => "stt",
        Capability::Tts => "tts",
        Capability::Vad => "vad",
        Capability::EndOfTurn => "end-of-turn",
        _ => "unknown",
    }
}

pub(crate) fn status(status: &CatalogStatus) -> StatusView {
    StatusView { reason: status.reason.as_ref().map(reason), stale: status.stale, detail: status.detail.clone() }
}

fn reason(reason: &Reason) -> ReasonView {
    ReasonView { code: reason.code.to_string(), params: ReasonParams { needs: reason.needs, has: reason.has } }
}

fn voice(voice: &Voice) -> VoiceView {
    VoiceView {
        id: voice.id.clone(),
        name: voice.name.clone(),
        languages: voice.languages.clone(),
        gender: voice.gender.map(|gender| match gender {
            Gender::Female => "female",
            Gender::Male => "male",
        }),
    }
}

fn speed(range: SpeedRange) -> SpeedView {
    SpeedView { min: range.min, max: range.max }
}

fn build(build: &ModelBuild) -> BuildView {
    BuildView {
        id: build.id.clone(),
        backend: build.backend.clone(),
        accelerator: build.accelerator.map(accelerator_name),
        precision: build.precision.clone(),
        download_bytes: build.download_bytes,
        memory_mb: build.memory_mb,
        available: build.available,
        reasons: build.reasons.iter().map(reason).collect(),
        installed: build.installed,
    }
}

/// A local model, with its family, builds and what is installed.
pub(crate) fn local_model(model: &LocalModelInfo) -> ModelView {
    ModelView {
        id: model.id.clone(),
        capabilities: model.capabilities.iter().copied().map(capability_name).collect(),
        languages: model.languages.clone(),
        voices: model.voices.iter().map(voice).collect(),
        speed: model.speed.map(speed),
        local: Some(LocalView {
            family: model.family.clone(),
            parameters_m: model.parameters_m,
            license: model.license.clone(),
            installed: model.installed,
            builds: model.builds.iter().map(build).collect(),
            recommended_build: model.recommended_build.clone(),
        }),
    }
}

/// A remote provider's model, as listed.
pub(crate) fn remote_model(model: &dyn ModelInfo) -> ModelView {
    ModelView {
        id: model.id().to_string(),
        capabilities: model.capabilities().iter().copied().map(capability_name).collect(),
        languages: model.languages().to_vec(),
        voices: model.voices().iter().map(voice).collect(),
        speed: model.speed().map(speed),
        local: None,
    }
}
