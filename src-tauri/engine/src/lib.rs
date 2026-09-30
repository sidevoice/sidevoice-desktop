//! Native model engines, downloaded at run time rather than compiled into the app (docs/ENGINES.md).
//!
//! `NativeEngines` answers the three things a client asks: what can this device run (offers, from the
//! catalog and the device's capabilities), make one ready (download the engine package and the model, both
//! checked against the catalog's SHA-256), and run it (transcribe, synthesize). Tonight one engine exists,
//! sherpa-onnx, for Apple Silicon; the shape is the catalog's, not this engine's.

pub mod install;
pub mod sherpa;
pub mod sherpa_ffi;

use install::Store;
use sherpa::{KokoroFiles, Recognizer, Sherpa, Tts, WhisperFiles};
use sidevoice_desktop_core::engines::{self, Capability, Catalog, Device, Model, Runs, Task};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub type Error = String;

pub struct NativeEngines {
    pub catalog: Catalog,
    pub device: Device,
    store: Store,
    /// `cpu` or `coreml`; `SIDEVOICE_ENGINE_PROVIDER` overrides (CoreML is not the default until measured).
    provider: String,
    sherpa: Mutex<Option<Arc<Sherpa>>>,
    /// One install at a time: two would download into the same staging paths and break each other.
    installing: Mutex<()>,
    recognizers: Mutex<HashMap<(String, String), Arc<Recognizer>>>,
    voices: Mutex<HashMap<(String, String), Arc<Tts>>>,
}

/// A native offer as the page reads it (docs/ENGINES.md → "The page's side"): the model, its best build and
/// accelerator, plus whether its downloads are already on this device.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Available {
    pub model: String,
    pub engine: String,
    pub task: Task,
    pub label: String,
    pub languages: Vec<String>,
    pub runs: Runs,
    /// Bytes to download before first use: the engine package plus the model's files.
    pub download_size: u64,
    /// The one the resolver chose, and every one this build can use here, best first.
    pub accelerator: Capability,
    pub accelerators: Vec<Capability>,
    pub reason: String,
    pub installed: bool,
    pub voices: Vec<engines::Voice>,
}

impl NativeEngines {
    pub fn new(catalog: Catalog, root: impl Into<PathBuf>) -> Self {
        let provider = std::env::var("SIDEVOICE_ENGINE_PROVIDER").unwrap_or_else(|_| "cpu".into());
        NativeEngines {
            catalog,
            device: engines::native_device(),
            store: Store::new(root),
            provider,
            sherpa: Mutex::default(),
            installing: Mutex::default(),
            recognizers: Mutex::default(),
            voices: Mutex::default(),
        }
    }

    /// What this app runs natively: a native device is never offered a page engine's builds (the page knows its own).
    pub fn available(&self) -> Vec<Available> {
        engines::offers(&self.catalog, &self.device, "device")
            .unwrap_or_default()
            .into_iter()
            .filter_map(|offer| {
                let model = self.model(&offer.model)?;
                let installed = self.paths(&offer.model, &offer.engine).map(|(e, m)| e.1 && m.1).unwrap_or(false);
                let accelerators = std::iter::once(offer.accelerator)
                    .chain(offer.alternatives.iter().filter(|a| a.engine == offer.engine).map(|a| a.accelerator))
                    .collect();
                Some(Available {
                    label: model.label.clone(),
                    languages: model.languages.clone(),
                    runs: Runs::Native,
                    voices: model.voices.clone(),
                    installed,
                    accelerators,
                    model: offer.model,
                    engine: offer.engine,
                    task: offer.task,
                    download_size: offer.download_size,
                    accelerator: offer.accelerator,
                    reason: offer.reason,
                })
            })
            .collect()
    }

    fn model(&self, id: &str) -> Option<&Model> {
        self.catalog.model(id)
    }

    /// ((engine dir, installed), (model dir, installed)) for a native model on an engine this device runs.
    #[allow(clippy::type_complexity)]
    fn paths(&self, model_id: &str, engine_id: &str) -> Result<((PathBuf, bool), (PathBuf, bool)), Error> {
        let engine = self.catalog.engine(engine_id).ok_or("unknown engine")?;
        let package = engines::package_for(engine, &self.device).ok_or("this engine has no package for this device")?;
        let model = self.model(model_id).ok_or("unknown model")?;
        let build = model.build(engine_id).ok_or("this model does not run on that engine")?;
        let download = build.download.as_ref().ok_or("this build has nothing to download")?;
        let (arch, package_download) = downloaded(package)?;
        let engine_dir = self.store.engine_dir(&engine.id, &engine.version, &package.os, arch, package_download);
        let model_dir = self.store.model_dir(&engine.id, &model.id, download);
        Ok((
            (engine_dir.clone(), Store::installed(&engine_dir, package_download)),
            (model_dir.clone(), Store::installed(&model_dir, download)),
        ))
    }

    /// Downloads what `model` on `engine` needs, reporting (done, total) bytes across both downloads.
    pub fn install(&self, model_id: &str, engine_id: &str, progress: &mut dyn FnMut(u64, u64)) -> Result<(), Error> {
        let _one_at_a_time = self.installing.lock().map_err(|_| "poisoned")?;
        let engine = self.catalog.engine(engine_id).ok_or("unknown engine")?;
        let package = engines::package_for(engine, &self.device).ok_or("no package for this device")?;
        let model = self.model(model_id).ok_or("unknown model")?;
        let download = model.build(engine_id).and_then(|b| b.download.as_ref()).ok_or("nothing to download")?;
        let (_, package_download) = downloaded(package)?;
        let ((engine_dir, engine_ok), (model_dir, model_ok)) = self.paths(model_id, engine_id)?;
        let total = if engine_ok { 0 } else { package_download.size } + if model_ok { 0 } else { download.size };
        let mut base = 0;
        if !engine_ok {
            self.store.fetch(package_download, &engine_dir, &mut |done, _| progress(done, total))?;
            base = package_download.size;
        }
        if !model_ok {
            self.store.fetch(download, &model_dir, &mut |done, _| progress(base + done, total))?;
        }
        Ok(())
    }

    fn engine(&self, engine_id: &str) -> Result<Arc<Sherpa>, Error> {
        if engine_id != "sherpa-onnx" {
            return Err(format!("no native runtime for {engine_id} in this app"));
        }
        let mut loaded = self.sherpa.lock().map_err(|_| "poisoned")?;
        if let Some(engine) = loaded.as_ref() {
            return Ok(engine.clone());
        }
        let engine = self.catalog.engine(engine_id).ok_or("unknown engine")?;
        let package = engines::package_for(engine, &self.device).ok_or("no package for this device")?;
        let (arch, package_download) = downloaded(package)?;
        let dir = self.store.engine_dir(&engine.id, &engine.version, &package.os, arch, package_download);
        if !Store::installed(&dir, package_download) {
            return Err("the engine is not downloaded yet".into());
        }
        let sherpa = Arc::new(Sherpa::load(&dir, &package.libraries)?);
        *loaded = Some(sherpa.clone());
        Ok(sherpa)
    }

    fn ready_model(&self, model_id: &str, engine_id: &str, task: Task) -> Result<(PathBuf, serde_json::Value), Error> {
        let model = self.model(model_id).ok_or("unknown model")?;
        if self.catalog.task_of(model) != Some(task) {
            return Err(format!("{model_id} is not a {task:?} model"));
        }
        let (_, (dir, ok)) = self.paths(model_id, engine_id)?;
        if !ok {
            return Err(format!("{model_id} is not downloaded yet"));
        }
        Ok((dir, model.build(engine_id).ok_or("this model does not run on that engine")?.config.clone()))
    }

    /// Mono f32 samples at `sample_rate` → text. `language`: Whisper's code, or empty to detect it.
    pub fn transcribe(
        &self,
        model_id: &str,
        language: &str,
        samples: &[f32],
        sample_rate: i32,
    ) -> Result<String, Error> {
        let engine_id = "sherpa-onnx";
        let language = engines::whisper_language(language)
            .ok_or_else(|| format!("Whisper does not know the language {language:?}"))?
            .to_string();
        let key = (model_id.to_string(), language.clone());
        let existing = self.recognizers.lock().map_err(|_| "poisoned")?.get(&key).cloned();
        let recognizer = match existing {
            Some(r) => r,
            None => {
                let (dir, config) = self.ready_model(model_id, engine_id, Task::Stt)?;
                let files: WhisperFiles = serde_json::from_value(config).map_err(|e| e.to_string())?;
                let r =
                    Arc::new(Recognizer::whisper(self.engine(engine_id)?, &dir, &files, &language, &self.provider, 4)?);
                self.recognizers.lock().map_err(|_| "poisoned")?.insert(key, r.clone());
                r
            }
        };
        recognizer.transcribe(samples, sample_rate)
    }

    /// Text → mono f32 samples. `voice`: a voice id the catalog lists for the model (its language picks the
    /// phonemizer).
    pub fn synthesize(&self, model_id: &str, voice: &str, speed: f32, text: &str) -> Result<sherpa::Audio, Error> {
        let engine_id = "sherpa-onnx";
        let model = self.model(model_id).ok_or("unknown model")?;
        let chosen = model.voices.iter().find(|v| v.id == voice).ok_or_else(|| format!("unknown voice {voice}"))?;
        let key = (model_id.to_string(), chosen.language.clone());
        let existing = self.voices.lock().map_err(|_| "poisoned")?.get(&key).cloned();
        let tts = match existing {
            Some(t) => t,
            None => {
                let (dir, config) = self.ready_model(model_id, engine_id, Task::Tts)?;
                let files: KokoroFiles = serde_json::from_value(config).map_err(|e| e.to_string())?;
                let t =
                    Arc::new(Tts::kokoro(self.engine(engine_id)?, &dir, &files, &chosen.language, &self.provider, 4)?);
                self.voices.lock().map_err(|_| "poisoned")?.insert(key, t.clone());
                t
            }
        };
        tts.synthesize(text, chosen.sid, if speed > 0.0 { speed } else { 1.0 })
    }

    pub fn accelerators(&self) -> Vec<Capability> {
        self.device.capabilities.clone()
    }
}

/// A downloaded package's architecture and download. This app downloads its engines; a bundled one (mobile) has
/// neither, and is not this app's to install.
fn downloaded(package: &engines::Package) -> Result<(&str, &engines::Download), Error> {
    match (package.arch.as_deref(), package.download.as_ref()) {
        (Some(arch), Some(download)) if !package.bundled => Ok((arch, download)),
        _ => Err("this engine package is bundled, not downloaded".into()),
    }
}
