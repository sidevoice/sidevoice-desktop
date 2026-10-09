//! The native engine, as the bundled interface reaches it (docs/BRIDGE.md → "The native engine").
//!
//! The bridge script exposes these as `window.__sidevoiceDesktop.host.nativeEngine`. Only the room window's own
//! pages may call them (capabilities/room.json); the settings window may read what is on disk
//! (`engine_on_disk`, capabilities/settings.json). Engine work runs on blocking threads, never the main one.
//! Audio crosses as raw bytes: little-endian f32 samples (transcribe's body; synthesize's answer, after a
//! 4-byte little-endian sample rate). A refusal is the engine's keyed `Error` (docs/BRIDGE.md → "Refusals").
//! What stays in memory follows sidevoice/sidevoice-core#21 D13 (`call_changed`, `unload_when_idle`).

use sidevoice_desktop_engine::error::{bad_request, internal};
use sidevoice_desktop_engine::{Device, Error, Installed, Jobs, Load, Loaded, Memory, NativeEngines, OnDisk, Progress};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::ipc::{InvokeBody, Request, Response};
use tauri::{AppHandle, Manager, State};

pub struct EngineState {
    pub engines: Arc<NativeEngines>,
    /// Each install in flight, by the page's job id: its progress, and its cancel.
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
pub async fn engine_installed(state: State<'_, EngineState>) -> Result<Vec<Installed>, Error> {
    let engines = state.engines.clone();
    tauri::async_runtime::spawn_blocking(move || engines.installed()).await.map_err(internal)?
}

/// `[{id, capabilities, languages, voices?, builds: [{id, backend, accelerator, available}]}]`: the models this app
/// offers, for the page's report to the core (sidevoice-core#85; the page adds `version` and `defaults`).
#[tauri::command]
pub async fn engine_models(state: State<'_, EngineState>) -> Result<Vec<serde_json::Value>, Error> {
    let engines = state.engines.clone();
    tauri::async_runtime::spawn_blocking(move || engines.models_report()).await.map_err(internal)?
}

/// For the settings window: engine packages and model builds on disk, with their sizes.
#[tauri::command]
pub async fn engine_on_disk(state: State<'_, EngineState>) -> Result<OnDisk, Error> {
    let engines = state.engines.clone();
    tauri::async_runtime::spawn_blocking(move || engines.on_disk()).await.map_err(internal)?
}

/// Installs `model` on `engine` as `job` (the page's id for this call): its progress while it runs, cancellable with
/// `engine_cancel` until it ends.
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

/// `{job, model, engine, done, total, bytes_per_s}` of install `job` once it has started; `null` while it waits or
/// after it ends.
#[tauri::command]
pub fn engine_progress(state: State<'_, EngineState>, job: String) -> Option<Progress> {
    state.jobs.get(&job)
}

/// Cancels install `job`, waiting or running: it removes what it was downloading and rejects with
/// `install_cancelled`. True when the job was running or waiting here.
#[tauri::command]
pub fn engine_cancel(state: State<'_, EngineState>, job: String) -> bool {
    state.jobs.cancel(&job)
}

fn header<'a>(request: &'a Request<'_>, name: &str) -> Result<&'a str, Error> {
    let value = request.headers().get(name).and_then(|v| v.to_str().ok());
    value.ok_or_else(|| bad_request(format!("missing header {name}")))
}

/// An accelerator as the catalogue names it (`cpu`, `metal`…); empty or absent for the engine's choice. One the
/// build does not run on here is refused where the choice is checked.
fn accelerator(name: Option<&str>) -> Option<String> {
    name.map(str::trim).filter(|n| !n.is_empty()).map(str::to_string)
}

fn samples_of(bytes: &[u8]) -> Vec<f32> {
    bytes.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()
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
    let rate: u32 = header(&request, "x-sample-rate")?.parse().map_err(|_| bad_request("bad x-sample-rate"))?;
    let engines = state.engines.clone();
    tauri::async_runtime::spawn_blocking(move || {
        engines.transcribe(&model, &engine, accelerator.as_deref(), &language, &samples, rate)
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
        engines.synthesize(&model, &engine, accelerator.as_deref(), &voice, speed, &text)
    })
    .await
    .map_err(internal)??;
    let mut bytes = Vec::with_capacity(4 + audio.samples.len() * 4);
    bytes.extend_from_slice(&audio.sample_rate.to_le_bytes());
    for sample in audio.samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(Response::new(bytes))
}

/// Loads `model` on `engine` into memory, on `accelerator` (else the one the engine runs it on here): `{load_ms}`.
#[tauri::command]
pub async fn engine_load(
    state: State<'_, EngineState>,
    model: String,
    engine: String,
    accelerator: Option<String>,
) -> Result<Load, Error> {
    let engines = state.engines.clone();
    let accelerator = self::accelerator(accelerator.as_deref());
    tauri::async_runtime::spawn_blocking(move || engines.load(&model, &engine, accelerator.as_deref()))
        .await
        .map_err(internal)?
}

/// Frees `model` on `engine` on `accelerator`, or on every accelerator when it is `null`; nothing to do when it is not
/// loaded. A load of it still under way is not kept.
#[tauri::command]
pub fn engine_unload(state: State<'_, EngineState>, model: String, engine: String, accelerator: Option<String>) {
    state.engines.unload(&model, &engine, self::accelerator(accelerator.as_deref()).as_deref());
}

/// `[{model, engine, accelerator, since, last_used}]`: the models in memory.
#[tauri::command]
pub fn engine_loaded(state: State<'_, EngineState>) -> Vec<Loaded> {
    state.engines.loaded()
}

/// `{total_mb, available_mb}`.
#[tauri::command]
pub fn engine_memory(state: State<'_, EngineState>) -> Memory {
    state.engines.memory()
}

/// sidevoice/sidevoice-core#21 D13, from the room's call state (`bridge_state`): as a call connects, what the app
/// unloaded while idle is loaded again, off the main thread.
pub fn call_changed(app: &AppHandle, joined: bool) {
    let Some(state) = app.try_state::<EngineState>() else { return };
    let engines = state.engines.clone();
    if !engines.call_changed(joined, Instant::now()) {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        for (model, result) in engines.preload() {
            let what = format!("{}@{}/{}", model.model, model.engine, model.accelerator);
            match result {
                Ok(load) => crate::debug(&format!("engine preload {what} load_ms={}", load.load_ms)),
                Err(e) => crate::debug(&format!("engine preload {what} refused: {}", e.key)),
            }
        }
    });
}

/// D13: with no call on, unloads what has gone unused for `idle_unload`; checked a few times per period.
pub fn unload_when_idle(engines: Arc<NativeEngines>) {
    let every = (engines.idle_unload / 4).clamp(Duration::from_secs(1), Duration::from_secs(30));
    std::thread::spawn(move || loop {
        std::thread::sleep(every);
        for model in engines.unload_idle(Instant::now()) {
            crate::debug(&format!("engine unloaded idle {}@{}/{}", model.model, model.engine, model.accelerator));
        }
    });
}
