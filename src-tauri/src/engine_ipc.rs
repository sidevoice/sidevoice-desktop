//! The native engine, as the bundled interface reaches it (docs/ENGINES.md → "The page's side").
//!
//! The bridge script exposes these as `window.__sidevoiceDesktop.host.nativeEngine`. Only the room window's own
//! pages may call them (capabilities/room.json). Engine work runs on blocking threads, never the main one.
//! Audio crosses as raw bytes: little-endian f32 samples (transcribe's body; synthesize's answer, after a
//! 4-byte little-endian sample rate).

use sidevoice_desktop_engine::{Available, NativeEngines};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::ipc::{InvokeBody, Request, Response};
use tauri::State;

pub struct EngineState {
    pub engines: Arc<NativeEngines>,
    /// Bytes downloaded and expected by the install in progress (one at a time is enough for a person).
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
}

impl EngineState {
    pub fn new(engines: NativeEngines) -> Self {
        EngineState { engines: Arc::new(engines), done: Arc::default(), total: Arc::default() }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfo {
    device: sidevoice_desktop_core::engines::Device,
    offers: Vec<Available>,
    /// Model ids as the page's browser engine names them (`builds["transformers-js"].config.repository`), so the
    /// page can say "this same model, natively".
    page_ids: std::collections::BTreeMap<String, String>,
}

#[tauri::command]
pub fn engine_available(state: State<'_, EngineState>) -> EngineInfo {
    let engines = &state.engines;
    let page_ids = engines
        .catalog
        .models
        .iter()
        .filter_map(|m| {
            let repository = m.build("transformers-js")?.config.get("repository")?.as_str()?;
            Some((repository.to_string(), m.id.clone()))
        })
        .collect();
    EngineInfo { device: engines.device.clone(), offers: engines.available(), page_ids }
}

#[tauri::command]
pub async fn engine_install(state: State<'_, EngineState>, model: String, engine: String) -> Result<(), String> {
    let engines = state.engines.clone();
    let (done, total) = (state.done.clone(), state.total.clone());
    done.store(0, Ordering::Relaxed);
    total.store(0, Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || {
        engines.install(&model, &engine, &mut |d, t| {
            done.store(d, Ordering::Relaxed);
            total.store(t, Ordering::Relaxed);
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// `[done, total]` bytes of the install in progress.
#[tauri::command]
pub fn engine_progress(state: State<'_, EngineState>) -> [u64; 2] {
    [state.done.load(Ordering::Relaxed), state.total.load(Ordering::Relaxed)]
}

fn header<'a>(request: &'a Request<'_>, name: &str) -> Result<&'a str, String> {
    request.headers().get(name).and_then(|v| v.to_str().ok()).ok_or_else(|| format!("missing header {name}"))
}

fn samples_of(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// Body: f32 samples. Headers: `x-model`, `x-language` (empty = detect), `x-sample-rate`. Answer: the text.
#[tauri::command]
pub async fn engine_transcribe(state: State<'_, EngineState>, request: Request<'_>) -> Result<String, String> {
    let InvokeBody::Raw(bytes) = request.body() else { return Err("send the audio as raw bytes".into()) };
    let samples = samples_of(bytes);
    let model = header(&request, "x-model")?.to_string();
    let language = header(&request, "x-language").unwrap_or("").to_string();
    let rate: i32 = header(&request, "x-sample-rate")?.parse().map_err(|_| "bad x-sample-rate")?;
    let engines = state.engines.clone();
    tauri::async_runtime::spawn_blocking(move || engines.transcribe(&model, &language, &samples, rate))
        .await
        .map_err(|e| e.to_string())?
}

/// Answer: 4 bytes of sample rate (u32 LE), then f32 LE samples.
#[tauri::command]
pub async fn engine_synthesize(
    state: State<'_, EngineState>,
    model: String,
    voice: String,
    speed: f32,
    text: String,
) -> Result<Response, String> {
    let engines = state.engines.clone();
    let audio = tauri::async_runtime::spawn_blocking(move || engines.synthesize(&model, &voice, speed, &text))
        .await
        .map_err(|e| e.to_string())??;
    let mut bytes = Vec::with_capacity(4 + audio.samples.len() * 4);
    bytes.extend_from_slice(&(audio.sample_rate as u32).to_le_bytes());
    for sample in audio.samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(Response::new(bytes))
}
