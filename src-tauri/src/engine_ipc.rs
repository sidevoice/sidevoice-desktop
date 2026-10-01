//! The native engine, as the bundled interface reaches it (docs/BRIDGE.md → "The native engine").
//!
//! The bridge script exposes these as `window.__sidevoiceDesktop.host.nativeEngine`. Only the room window's own
//! pages may call them (capabilities/room.json); the settings window may read what is on disk
//! (`engine_on_disk`, capabilities/settings.json). Engine work runs on blocking threads, never the main one.
//! Audio crosses as raw bytes: little-endian f32 samples (transcribe's body; synthesize's answer, after a
//! 4-byte little-endian sample rate). A refusal is the engine's keyed `Error` (docs/BRIDGE.md → "Refusals").

use sidevoice_desktop_core::engines::{self, Capability, Device};
use sidevoice_desktop_engine::error::{bad_request, internal};
use sidevoice_desktop_engine::{Error, Installed, Jobs, NativeEngines, OnDisk};
use std::sync::Arc;
use tauri::ipc::{InvokeBody, Request, Response};
use tauri::State;

pub struct EngineState {
    pub engines: Arc<NativeEngines>,
    /// Progress of each install in flight, by the page's job id.
    jobs: Arc<Jobs>,
}

impl EngineState {
    pub fn new(engines: NativeEngines) -> Self {
        EngineState { engines: Arc::new(engines), jobs: Arc::default() }
    }
}

/// `{runs: "native", os, arch, has, memory_mb}`: what the page resolves its offers from.
#[tauri::command]
pub fn engine_capabilities(state: State<'_, EngineState>) -> Device {
    state.engines.device.clone()
}

/// `[{model, engine}]`: the builds on disk.
#[tauri::command]
pub fn engine_installed(state: State<'_, EngineState>) -> Vec<Installed> {
    state.engines.installed()
}

/// For the settings window: engine packages and model builds on disk, with their sizes.
#[tauri::command]
pub fn engine_on_disk(state: State<'_, EngineState>) -> OnDisk {
    state.engines.on_disk()
}

/// Installs `model` on `engine`; its progress is `job`'s (the page's id for this call) while it runs.
#[tauri::command]
pub async fn engine_install(
    state: State<'_, EngineState>,
    model: String,
    engine: String,
    job: String,
) -> Result<(), Error> {
    let (engines, jobs) = (state.engines.clone(), state.jobs.clone());
    tauri::async_runtime::spawn_blocking(move || engines.install_job(&jobs, &job, &model, &engine))
        .await
        .map_err(internal)?
}

/// `[done, total]` bytes of install `job` once it has started, `null` while it waits or after it ends.
#[tauri::command]
pub fn engine_progress(state: State<'_, EngineState>, job: String) -> Option<[u64; 2]> {
    state.jobs.get(&job)
}

fn header<'a>(request: &'a Request<'_>, name: &str) -> Result<&'a str, Error> {
    let value = request.headers().get(name).and_then(|v| v.to_str().ok());
    value.ok_or_else(|| bad_request(format!("missing header {name}")))
}

/// An accelerator as the catalogue names it (`cpu`, `coreml`…); empty or absent for the resolver's choice. One
/// this app does not know parses as unknown, which no build can use: refused where the choice is checked.
fn accelerator(name: Option<&str>) -> Option<Capability> {
    let name = name.map(str::trim).filter(|n| !n.is_empty())?;
    Some(engines::capability_named(name))
}

fn samples_of(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// Body: f32 samples. Headers: `x-model`, `x-engine`, `x-accelerator` (optional), `x-language` (empty = detect),
/// `x-sample-rate`. Answer: the text.
#[tauri::command]
pub async fn engine_transcribe(state: State<'_, EngineState>, request: Request<'_>) -> Result<String, Error> {
    let InvokeBody::Raw(bytes) = request.body() else { return Err(bad_request("send the audio as raw bytes")) };
    let samples = samples_of(bytes);
    let model = header(&request, "x-model")?.to_string();
    let engine = header(&request, "x-engine")?.to_string();
    let accelerator = accelerator(header(&request, "x-accelerator").ok());
    let language = header(&request, "x-language").unwrap_or("").to_string();
    let rate: i32 = header(&request, "x-sample-rate")?.parse().map_err(|_| bad_request("bad x-sample-rate"))?;
    let engines = state.engines.clone();
    tauri::async_runtime::spawn_blocking(move || {
        engines.transcribe(&model, &engine, accelerator, &language, &samples, rate)
    })
    .await
    .map_err(internal)?
}

/// Answer: 4 bytes of sample rate (u32 LE), then f32 LE samples.
#[tauri::command]
pub async fn engine_synthesize(
    state: State<'_, EngineState>,
    model: String,
    engine: String,
    accelerator: Option<String>,
    voice: String,
    speed: f32,
    text: String,
) -> Result<Response, Error> {
    let engines = state.engines.clone();
    let accelerator = self::accelerator(accelerator.as_deref());
    let audio = tauri::async_runtime::spawn_blocking(move || {
        engines.synthesize(&model, &engine, accelerator, &voice, speed, &text)
    })
    .await
    .map_err(internal)??;
    let mut bytes = Vec::with_capacity(4 + audio.samples.len() * 4);
    bytes.extend_from_slice(&(audio.sample_rate as u32).to_le_bytes());
    for sample in audio.samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(Response::new(bytes))
}
