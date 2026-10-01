//! Native model engines, downloaded at run time rather than compiled into the app (docs/ENGINES.md).
//!
//! `NativeEngines` answers what the page asks through the bridge (docs/BRIDGE.md): what this device is (its
//! capabilities: the page resolves its offers from them and the catalogue), which builds are on disk, make one
//! ready (download the engine package and the model's build, both checked against the catalogue's SHA-256), and run
//! it. It always runs the build the page chose — catalogue model id + engine id — with the accelerator the page
//! chose or, when it names none, the first that build can use here. Before it downloads or runs anything it checks,
//! at this trust boundary, that the build runs here: an adapter this app has (`ADAPTERS`), a package for this
//! device, the build's needs and accelerators, the model's memory. Anything else is refused with a keyed `Error`,
//! never replaced by another build.

pub mod error;
pub mod install;
pub mod memory;
pub mod runanywhere;
pub mod sherpa;
pub mod sherpa_ffi;

pub use error::Error;
use install::Store;
use sidevoice_desktop_core::engines::{self, Build, Capability, Catalog, Device, Download, Engine, Model, Package};
use sidevoice_desktop_core::engines::{Runs, Task};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// Mono audio in [-1, 1].
pub struct Audio {
    pub samples: Vec<f32>,
    pub sample_rate: i32,
}

/// A native engine as this app runs it, loaded from its unpacked package: it turns a model build (its files in
/// `dir`, the engine's `config` from the catalogue) into something that runs, on the accelerator it is given.
pub trait Runtime: Send + Sync {
    /// Speech to text. `language`: the model family's code, or empty to detect it.
    fn recognizer(
        &self,
        family: &str,
        dir: &Path,
        config: &serde_json::Value,
        language: &str,
        accelerator: Capability,
    ) -> Result<Box<dyn Recognize>, Error>;

    /// Text to speech. `language`: the chosen voice's, for the phonemizer.
    fn voice(
        &self,
        family: &str,
        dir: &Path,
        config: &serde_json::Value,
        language: &str,
        accelerator: Capability,
    ) -> Result<Box<dyn Speak>, Error>;
}

pub trait Recognize: Send + Sync {
    /// Mono samples at `sample_rate` → text.
    fn transcribe(&self, samples: &[f32], sample_rate: i32) -> Result<String, Error>;
}

pub trait Speak: Send + Sync {
    /// Text → audio, with the model's speaker `sid`.
    fn synthesize(&self, text: &str, sid: i32, speed: f32) -> Result<Audio, Error>;
}

/// Loads an engine's runtime from the root of its unpacked package.
pub type Loader = fn(&Path, &Package) -> Result<Arc<dyn Runtime>, Error>;

/// The files a model build of a family must have in its directory, from its catalogue config.
pub type FilesOf = fn(&str, &serde_json::Value) -> Result<Vec<String>, Error>;

/// What this app knows how to run for one catalogue engine.
#[derive(Clone, Copy)]
pub struct Adapter {
    pub engine: &'static str,
    /// Loads the runtime from the root of the unpacked package.
    pub load: Loader,
    /// The files a model build of a family must have in its directory, from its catalogue config: a build missing
    /// one is not installed.
    pub files: FilesOf,
}

/// The native engines this app has an adapter for. Adding one is the one compiled part of adding an engine
/// (docs/ENGINES.md).
pub const ADAPTERS: &[Adapter] = &[
    Adapter { engine: "sherpa-onnx", load: sherpa::load, files: sherpa::files },
    // SPIKE (spike/runanywhere): RunAnywhere's RACommons, see runanywhere.rs.
    Adapter { engine: runanywhere::ENGINE, load: runanywhere::load, files: runanywhere::files },
];

/// A build on disk, as the page names it: catalogue model id + engine id.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Installed {
    pub model: String,
    pub engine: String,
}

/// What the engines take on disk, for the app's own settings window.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OnDisk {
    pub engines: Vec<PackageOnDisk>,
    pub builds: Vec<BuildOnDisk>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PackageOnDisk {
    pub engine: String,
    pub label: String,
    pub version: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BuildOnDisk {
    pub model: String,
    pub label: String,
    pub engine: String,
    pub task: Task,
    pub bytes: u64,
}

/// Progress of the installs in flight, one entry per job (the page's id for one `install` call), as
/// `[done, total]` bytes. A job appears when it starts — once it holds the install lock — and goes when it ends: a
/// job still waiting for another reports nothing, and no job ever reports another's bytes.
#[derive(Default)]
pub struct Jobs(Mutex<HashMap<String, [u64; 2]>>);

impl Jobs {
    fn entries(&self) -> MutexGuard<'_, HashMap<String, [u64; 2]>> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `[done, total]` of `job`, while it runs.
    pub fn get(&self, job: &str) -> Option<[u64; 2]> {
        self.entries().get(job).copied()
    }

    fn set(&self, job: &str, done: u64, total: u64) {
        self.entries().insert(job.to_string(), [done, total]);
    }

    fn end(&self, job: &str) {
        self.entries().remove(job);
    }
}

/// A loaded model: (engine, model, accelerator, language).
type Key = (String, String, Capability, String);

pub struct NativeEngines {
    pub catalog: Catalog,
    /// What `capabilities()` reports: this process's OS, architecture and accelerators, and the OS's memory.
    pub device: Device,
    store: Store,
    adapters: Vec<Adapter>,
    /// One loaded runtime per engine.
    runtimes: Mutex<HashMap<String, Arc<dyn Runtime>>>,
    /// One install at a time: two would download into the same staging paths and break each other.
    installing: Mutex<()>,
    recognizers: Mutex<HashMap<Key, Arc<dyn Recognize>>>,
    voices: Mutex<HashMap<Key, Arc<dyn Speak>>>,
}

/// Where a build lives on this device, once it is known this app can run it here.
struct Located<'a> {
    engine: &'a Engine,
    package: &'a Package,
    model: &'a Model,
    build: &'a Build,
    adapter: Adapter,
    /// What this build can use here, best first.
    accelerators: Vec<Capability>,
    engine_download: &'a Download,
    engine_dir: PathBuf,
    /// The package's libraries: what an installed package must have.
    engine_files: Vec<String>,
    model_download: &'a Download,
    model_dir: PathBuf,
    /// What the adapter needs from the model's files.
    model_files: Vec<String>,
}

impl Located<'_> {
    fn engine_installed(&self) -> bool {
        Store::installed(&self.engine_dir, self.engine_download, &self.engine_files)
    }

    fn model_installed(&self) -> bool {
        Store::installed(&self.model_dir, self.model_download, &self.model_files)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, Error> {
    mutex.lock().map_err(|_| error::internal("a lock was poisoned"))
}

impl NativeEngines {
    pub fn new(catalog: Catalog, root: impl Into<PathBuf>) -> Self {
        let mut device = engines::native_device();
        device.memory_mb = memory::total_mb();
        NativeEngines {
            catalog,
            device,
            store: Store::new(root),
            adapters: ADAPTERS.to_vec(),
            runtimes: Mutex::default(),
            installing: Mutex::default(),
            recognizers: Mutex::default(),
            voices: Mutex::default(),
        }
    }

    /// `model` on `engine`, if this app can run it on this device: an adapter for the engine, a native package for
    /// this OS/architecture, the model's memory (when both are known), a build of the model for that engine that
    /// fits here (its needs, an accelerator present), and downloads for both.
    fn locate(&self, model_id: &str, engine_id: &str) -> Result<Located<'_>, Error> {
        let adapter = *self
            .adapters
            .iter()
            .find(|a| a.engine == engine_id)
            .ok_or_else(|| error::engine_unsupported(engine_id))?;
        let engine = self.catalog.engine(engine_id).ok_or_else(|| error::engine_unsupported(engine_id))?;
        if engine.runs != Runs::Native {
            return Err(error::engine_unsupported(engine_id));
        }
        let platform = format!("{}-{}", self.device.os, self.device.arch);
        let package =
            engines::package_for(engine, &self.device).ok_or_else(|| error::engine_no_package(engine_id, &platform))?;
        let (arch, engine_download) =
            downloaded(package).ok_or_else(|| error::engine_no_package(engine_id, &platform))?;
        let model = self.catalog.model(model_id).ok_or_else(|| error::model_unknown(model_id))?;
        // Unknown memory is unknown, not too little: only a known shortfall refuses.
        if let (Some(needed), Some(memory)) = (model.requires.memory_mb, self.device.memory_mb) {
            if memory < needed {
                return Err(error::model_needs_memory(model_id, needed, memory));
            }
        }
        let build = model.build(engine_id).ok_or_else(|| error::build_missing(model_id, engine_id))?;
        let accelerators = engines::accelerators_for(&self.catalog, build, &self.device)
            .ok_or_else(|| error::build_unfit(model_id, engine_id))?;
        let model_download = build.download.as_ref().ok_or_else(|| error::build_unfit(model_id, engine_id))?;
        let model_files = (adapter.files)(&model.family, &build.config)?;
        Ok(Located {
            engine_dir: self.store.engine_dir(&engine.id, &engine.version, &package.os, arch, engine_download),
            engine_files: package.libraries.iter().map(|l| l.path.clone()).collect(),
            model_dir: self.store.model_dir(&engine.id, &model.id, model_download),
            model_files,
            engine,
            package,
            model,
            build,
            adapter,
            accelerators,
            engine_download,
            model_download,
        })
    }

    /// Every build this app can run here, in catalogue order.
    fn runnable(&self) -> impl Iterator<Item = Located<'_>> {
        self.catalog.models.iter().flat_map(move |m| m.builds.iter().filter_map(|b| self.locate(&m.id, &b.engine).ok()))
    }

    /// The builds whose engine package and model files are both on disk, whole.
    pub fn installed(&self) -> Vec<Installed> {
        self.runnable()
            .filter(|l| l.engine_installed() && l.model_installed())
            .map(|l| Installed { model: l.model.id.clone(), engine: l.engine.id.clone() })
            .collect()
    }

    /// Engine packages and model builds on disk, with the bytes each takes.
    pub fn on_disk(&self) -> OnDisk {
        let mut engines: Vec<PackageOnDisk> = Vec::new();
        let mut builds = Vec::new();
        for located in self.runnable() {
            if located.engine_installed() && !engines.iter().any(|e| e.engine == located.engine.id) {
                engines.push(PackageOnDisk {
                    engine: located.engine.id.clone(),
                    label: located.engine.label.clone(),
                    version: located.engine.version.clone(),
                    bytes: Store::bytes_on_disk(&located.engine_dir),
                });
            }
            if let (true, Some(task)) = (located.model_installed(), self.catalog.task_of(located.model)) {
                builds.push(BuildOnDisk {
                    model: located.model.id.clone(),
                    label: located.model.label.clone(),
                    engine: located.engine.id.clone(),
                    task,
                    bytes: Store::bytes_on_disk(&located.model_dir),
                });
            }
        }
        OnDisk { engines, builds }
    }

    /// Downloads what `model` on `engine` needs — repairing a download that lost a file — reporting (done, total)
    /// bytes across both downloads. The first report, `(0, total)`, comes once this install holds the install lock.
    pub fn install(&self, model_id: &str, engine_id: &str, progress: &mut dyn FnMut(u64, u64)) -> Result<(), Error> {
        let l = self.locate(model_id, engine_id)?;
        let _one_at_a_time = lock(&self.installing)?;
        let (engine_ok, model_ok) = (l.engine_installed(), l.model_installed());
        let total =
            if engine_ok { 0 } else { l.engine_download.size } + if model_ok { 0 } else { l.model_download.size };
        progress(0, total);
        let mut base = 0;
        if !engine_ok {
            self.store
                .fetch(l.engine_download, &l.engine_dir, &l.engine_files, &mut |done, _| progress(done, total))?;
            base = l.engine_download.size;
        }
        if !model_ok {
            let mut model_progress = |done, _| progress(base + done, total);
            self.store.fetch(l.model_download, &l.model_dir, &l.model_files, &mut model_progress)?;
        }
        Ok(())
    }

    /// `install`, its progress kept under `job` in `jobs` from the moment it starts until it ends.
    pub fn install_job(&self, jobs: &Jobs, job: &str, model_id: &str, engine_id: &str) -> Result<(), Error> {
        let result = self.install(model_id, engine_id, &mut |done, total| jobs.set(job, done, total));
        jobs.end(job);
        result
    }

    /// `model` on `engine`, downloaded and of `task`, with the accelerator it runs on: the one asked for, which must
    /// be one this build can use here, or the first it can.
    fn ready(
        &self,
        model_id: &str,
        engine_id: &str,
        task: Task,
        accelerator: Option<Capability>,
    ) -> Result<(Located<'_>, Capability), Error> {
        let located = self.locate(model_id, engine_id)?;
        if self.catalog.task_of(located.model) != Some(task) {
            let task = if task == Task::Stt { "speech-to-text" } else { "text-to-speech" };
            return Err(error::model_wrong_task(model_id, task));
        }
        let chosen = match accelerator {
            None => located.accelerators[0],
            Some(asked) if located.accelerators.contains(&asked) => asked,
            Some(asked) => {
                let usable = located.accelerators.iter().map(|a| engines::capability_name(*a)).collect();
                let asked = engines::capability_name(asked);
                let asked = if asked.is_empty() { "unknown".to_string() } else { asked };
                return Err(error::accelerator_unusable(model_id, engine_id, &asked, usable));
            }
        };
        if !located.engine_installed() || !located.model_installed() {
            return Err(error::not_installed(model_id, engine_id));
        }
        Ok((located, chosen))
    }

    /// The engine's runtime, loaded from its package on first use.
    fn runtime(&self, located: &Located) -> Result<Arc<dyn Runtime>, Error> {
        let mut loaded = lock(&self.runtimes)?;
        if let Some(runtime) = loaded.get(&located.engine.id) {
            return Ok(runtime.clone());
        }
        let runtime = (located.adapter.load)(&located.engine_dir, located.package)?;
        loaded.insert(located.engine.id.clone(), runtime.clone());
        Ok(runtime)
    }

    /// Mono f32 samples at `sample_rate` → text, with `model` on `engine`. `language`: the family's code, or empty
    /// to detect it.
    pub fn transcribe(
        &self,
        model_id: &str,
        engine_id: &str,
        accelerator: Option<Capability>,
        language: &str,
        samples: &[f32],
        sample_rate: i32,
    ) -> Result<String, Error> {
        let recognizer = self.recognizer_for(model_id, engine_id, accelerator, language)?;
        recognizer.transcribe(samples, sample_rate)
    }

    /// Text → mono f32 samples, with `model` on `engine`. `voice`: a voice id the catalogue lists for the model
    /// (its language picks the phonemizer).
    pub fn synthesize(
        &self,
        model_id: &str,
        engine_id: &str,
        accelerator: Option<Capability>,
        voice: &str,
        speed: f32,
        text: &str,
    ) -> Result<Audio, Error> {
        let (speaker, sid) = self.voice_for(model_id, engine_id, accelerator, voice)?;
        speaker.synthesize(text, sid, if speed > 0.0 { speed } else { 1.0 })
    }

    /// SPIKE: the phase-3 bridge's `load` — the recognizer (STT, `option` = language) or voice (TTS, `option` = voice
    /// id) in memory, ready for the first call. Returns how long loading took.
    pub fn load(
        &self,
        model_id: &str,
        engine_id: &str,
        accelerator: Option<Capability>,
        option: &str,
    ) -> Result<std::time::Duration, Error> {
        let started = std::time::Instant::now();
        let model = self.catalog.model(model_id).ok_or_else(|| error::model_unknown(model_id))?;
        if self.catalog.task_of(model) == Some(Task::Stt) {
            self.recognizer_for(model_id, engine_id, accelerator, option)?;
        } else {
            self.voice_for(model_id, engine_id, accelerator, option)?;
        }
        Ok(started.elapsed())
    }

    /// SPIKE: the phase-3 bridge's `unload` — drops every loaded instance of `model` on `engine` (a call holding one
    /// keeps it until it returns). Returns how many were dropped.
    pub fn unload(&self, model_id: &str, engine_id: &str) -> Result<usize, Error> {
        let matches = |k: &Key| k.0 == engine_id && k.1 == model_id;
        let mut recognizers = lock(&self.recognizers)?;
        let mut voices = lock(&self.voices)?;
        let before = recognizers.len() + voices.len();
        recognizers.retain(|k, _| !matches(k));
        voices.retain(|k, _| !matches(k));
        Ok(before - recognizers.len() - voices.len())
    }

    /// SPIKE: the phase-3 bridge's `loaded` — what is in memory, by model and engine.
    pub fn loaded(&self) -> Vec<Installed> {
        let mut keys: Vec<Key> = lock(&self.recognizers).map(|m| m.keys().cloned().collect()).unwrap_or_default();
        keys.extend(lock(&self.voices).map(|m| m.keys().cloned().collect::<Vec<_>>()).unwrap_or_default());
        let mut out: Vec<Installed> = keys.into_iter().map(|k| Installed { model: k.1, engine: k.0 }).collect();
        out.dedup();
        out
    }

    fn recognizer_for(
        &self,
        model_id: &str,
        engine_id: &str,
        accelerator: Option<Capability>,
        language: &str,
    ) -> Result<Arc<dyn Recognize>, Error> {
        let (located, accelerator) = self.ready(model_id, engine_id, Task::Stt, accelerator)?;
        let key = (engine_id.to_string(), model_id.to_string(), accelerator, language.trim().to_string());
        if let Some(r) = lock(&self.recognizers)?.get(&key).cloned() {
            return Ok(r);
        }
        let r: Arc<dyn Recognize> = Arc::from(self.runtime(&located)?.recognizer(
            &located.model.family,
            &located.model_dir,
            &located.build.config,
            &key.3,
            accelerator,
        )?);
        lock(&self.recognizers)?.insert(key, r.clone());
        Ok(r)
    }

    fn voice_for(
        &self,
        model_id: &str,
        engine_id: &str,
        accelerator: Option<Capability>,
        voice: &str,
    ) -> Result<(Arc<dyn Speak>, i32), Error> {
        let (located, accelerator) = self.ready(model_id, engine_id, Task::Tts, accelerator)?;
        let chosen = located.model.voices.iter().find(|v| v.id == voice);
        let chosen = chosen.ok_or_else(|| error::voice_unknown(model_id, voice))?;
        let key = (engine_id.to_string(), model_id.to_string(), accelerator, chosen.language.clone());
        if let Some(s) = lock(&self.voices)?.get(&key).cloned() {
            return Ok((s, chosen.sid));
        }
        let s: Arc<dyn Speak> = Arc::from(self.runtime(&located)?.voice(
            &located.model.family,
            &located.model_dir,
            &located.build.config,
            &chosen.language,
            accelerator,
        )?);
        lock(&self.voices)?.insert(key, s.clone());
        Ok((s, chosen.sid))
    }
}

/// A downloaded package's architecture and download. This app downloads its engines; a bundled one (mobile) has
/// neither, and is not this app's to install.
fn downloaded(package: &Package) -> Option<(&str, &Download)> {
    match (package.arch.as_deref(), package.download.as_ref()) {
        (Some(arch), Some(download)) if !package.bundled => Some((arch, download)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidevoice_desktop_core::engines::bundled_catalog;

    /// What the fake runtime was asked to load, in order, by every test: (model directory, "family model language
    /// accelerator").
    static LOADED: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());

    struct Fake;
    struct Heard(String);
    struct Said;

    impl Runtime for Fake {
        fn recognizer(
            &self,
            family: &str,
            dir: &Path,
            _config: &serde_json::Value,
            language: &str,
            accelerator: Capability,
        ) -> Result<Box<dyn Recognize>, Error> {
            let what = format!("{family} {} {language} {}", leaf(dir), engines::capability_name(accelerator));
            LOADED.lock().unwrap().push((dir.to_path_buf(), what.clone()));
            Ok(Box::new(Heard(what)))
        }

        fn voice(
            &self,
            family: &str,
            dir: &Path,
            _config: &serde_json::Value,
            language: &str,
            accelerator: Capability,
        ) -> Result<Box<dyn Speak>, Error> {
            LOADED.lock().unwrap().push((
                dir.to_path_buf(),
                format!("{family} {} {language} {}", leaf(dir), engines::capability_name(accelerator)),
            ));
            Ok(Box::new(Said))
        }
    }

    impl Recognize for Heard {
        fn transcribe(&self, _samples: &[f32], _rate: i32) -> Result<String, Error> {
            Ok(self.0.clone())
        }
    }

    impl Speak for Said {
        fn synthesize(&self, text: &str, sid: i32, _speed: f32) -> Result<Audio, Error> {
            Ok(Audio { samples: vec![0.0; text.len()], sample_rate: sid })
        }
    }

    fn leaf(dir: &Path) -> String {
        dir.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    const FAKE: Adapter = Adapter {
        engine: "sherpa-onnx",
        load: |_root, _package| Ok(Arc::new(Fake)),
        files: |_family, _config| Ok(vec!["model.onnx".into()]),
    };

    /// A complete download of `download` in `dir`, with `files`.
    fn mark(dir: &Path, download: &Download, files: &[String]) {
        for file in files {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, [0u8; 10]).unwrap();
        }
        std::fs::write(dir.parent().unwrap().join(install::MARKER), &download.sha256).unwrap();
    }

    fn mac_device(engines: &mut NativeEngines) {
        engines.device.os = "macos".into();
        engines.device.arch = "aarch64".into();
        engines.device.capabilities = vec![Capability::Cpu, Capability::Coreml, Capability::Metal, Capability::Mlx];
        engines.device.memory_mb = None; // what a test needs, it sets
    }

    /// An Apple Silicon Mac whose sherpa-onnx is the fake runtime, with the engine, Whisper tiny and Kokoro on disk.
    /// Downloads point at a port nothing listens on: an install that tries to download fails, fast, offline.
    fn mac(name: &str, change: impl FnOnce(&mut Catalog)) -> (NativeEngines, PathBuf) {
        let root = std::env::temp_dir().join(format!("sv-engines-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut catalog = bundled_catalog();
        for model in &mut catalog.models {
            for build in &mut model.builds {
                if let Some(download) = build.download.as_mut() {
                    download.url = format!("https://127.0.0.1:9/{}.tar.bz2", model.id);
                }
            }
        }
        change(&mut catalog);
        let mut engines = NativeEngines::new(catalog, &root);
        mac_device(&mut engines);
        engines.adapters = vec![FAKE];
        for model in ["whisper-tiny", "kokoro-82m-v1.0"] {
            let Ok(l) = engines.locate(model, "sherpa-onnx") else { continue };
            mark(&l.engine_dir, l.engine_download, &l.engine_files);
            mark(&l.model_dir, l.model_download, &l.model_files);
        }
        (engines, root)
    }

    #[test]
    fn runs_the_build_and_accelerator_it_is_asked_for_and_keys_its_cache_by_them() {
        let (engines, root) = mac("choice", |_| {});
        let heard = engines.transcribe("whisper-tiny", "sherpa-onnx", None, "es", &[0.0], 16_000).unwrap();
        assert_eq!(heard, "whisper whisper-tiny es cpu", "no accelerator named: the first the build can use here");
        let heard = engines.transcribe("whisper-tiny", "sherpa-onnx", Some(Capability::Coreml), "es", &[], 16_000);
        assert_eq!(heard.unwrap(), "whisper whisper-tiny es coreml", "the one asked for");
        engines.transcribe("whisper-tiny", "sherpa-onnx", None, "es", &[], 16_000).unwrap();
        let metal = engines.transcribe("whisper-tiny", "sherpa-onnx", Some(Capability::Metal), "es", &[], 16_000);
        let metal = metal.unwrap_err();
        assert_eq!(metal.key, "accelerator_unusable");
        assert_eq!(metal.params["accelerator"], "metal");
        assert_eq!(metal.params["usable"], serde_json::json!(["cpu", "coreml"]));
        let audio = engines.synthesize("kokoro-82m-v1.0", "sherpa-onnx", None, "ef_dora", 1.0, "hola").unwrap();
        assert_eq!(audio.samples.len(), 4);
        let loaded = LOADED.lock().unwrap().clone(); // every test's: this one's are under its own root
        let ours: Vec<&String> = loaded.iter().filter(|(dir, _)| dir.starts_with(&root)).map(|(_, l)| l).collect();
        assert_eq!(
            ours,
            ["whisper whisper-tiny es cpu", "whisper whisper-tiny es coreml", "kokoro kokoro-82m-v1.0 es cpu"],
            "each choice loaded once"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn what_does_not_run_here_is_refused_by_key_before_anything_is_downloaded_or_loaded() {
        let (engines, root) = mac("refuse", |_| {});
        let key = |r: Result<String, Error>| r.unwrap_err().key;
        let stt = |model: &str, engine: &str| engines.transcribe(model, engine, None, "es", &[], 16_000);
        assert_eq!(key(stt("whisper-tiny", "transformers-js")), "engine_unsupported", "a page engine");
        assert_eq!(engines.install("whisper-tiny", "mlx-audio", &mut |_, _| {}).unwrap_err().key, "engine_unsupported");
        assert_eq!(key(stt("whisper-nope", "sherpa-onnx")), "model_unknown");
        assert_eq!(key(stt("kokoro-82m-v1.0", "sherpa-onnx")), "model_wrong_task");
        assert_eq!(key(stt("whisper-base", "sherpa-onnx")), "not_installed");
        let voice = engines.synthesize("kokoro-82m-v1.0", "sherpa-onnx", None, "zz_nobody", 1.0, "x");
        assert_eq!(voice.err().unwrap().key, "voice_unknown");

        let mut linux = NativeEngines::new(engines.catalog.clone(), &root);
        linux.adapters = vec![FAKE];
        linux.device.os = "linux".into();
        let err = linux.install("whisper-tiny", "sherpa-onnx", &mut |_, _| {}).unwrap_err();
        assert_eq!(err.key, "engine_no_package");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_build_that_needs_what_this_device_lacks_is_unfit() {
        let (engines, root) = mac("unfit", |catalog| {
            let tiny = catalog.models.iter_mut().find(|m| m.id == "whisper-tiny").unwrap();
            tiny.builds.iter_mut().find(|b| b.engine == "sherpa-onnx").unwrap().needs = vec![Capability::Cuda];
        });
        let err = engines.install("whisper-tiny", "sherpa-onnx", &mut |_, _| {}).unwrap_err();
        assert_eq!((err.key, err.params["model"].clone()), ("build_unfit", serde_json::json!("whisper-tiny")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_model_needing_more_memory_than_this_device_has_is_refused_unknown_memory_is_not() {
        let (mut engines, root) = mac("memory", |catalog| {
            catalog.models.iter_mut().find(|m| m.id == "whisper-tiny").unwrap().requires.memory_mb = Some(4096);
        });
        engines.device.memory_mb = Some(1024);
        let err = engines.transcribe("whisper-tiny", "sherpa-onnx", None, "es", &[], 16_000).unwrap_err();
        assert_eq!(err.key, "model_needs_memory");
        assert_eq!((err.params["needed_mb"].clone(), err.params["memory_mb"].clone()), (4096.into(), 1024.into()));
        let install = engines.install("whisper-tiny", "sherpa-onnx", &mut |_, _| {}).unwrap_err();
        assert_eq!(install.key, "model_needs_memory", "refused before any download");
        assert!(!engines.installed().iter().any(|b| b.model == "whisper-tiny"));

        engines.device.memory_mb = Some(8192);
        assert!(engines.transcribe("whisper-tiny", "sherpa-onnx", None, "es", &[], 16_000).is_ok());
        engines.device.memory_mb = None;
        assert!(engines.transcribe("whisper-tiny", "sherpa-onnx", None, "es", &[], 16_000).is_ok(), "unknown ≠ short");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn installed_lists_the_builds_on_disk_by_model_and_engine() {
        let (engines, root) = mac("installed", |_| {});
        let installed = engines.installed();
        let expected = |model: &str| Installed { model: model.into(), engine: "sherpa-onnx".into() };
        assert_eq!(installed, vec![expected("whisper-tiny"), expected("kokoro-82m-v1.0")]);
        let disk = engines.on_disk();
        assert_eq!(disk.engines.len(), 1);
        assert_eq!(disk.builds.len(), 2);
        assert!(disk.builds.iter().all(|b| b.bytes >= 10));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_build_that_lost_a_file_is_not_installed_and_install_downloads_it_again() {
        let (engines, root) = mac("repair", |_| {});
        let tiny = engines.locate("whisper-tiny", "sherpa-onnx").unwrap();
        std::fs::remove_file(tiny.model_dir.join("model.onnx")).unwrap();
        let (model_dir, model_size) = (tiny.model_dir.clone(), tiny.model_download.size);
        assert!(!engines.installed().iter().any(|b| b.model == "whisper-tiny"), "the marker alone is not enough");
        assert!(!engines.on_disk().builds.iter().any(|b| b.model == "whisper-tiny"));
        let run = engines.transcribe("whisper-tiny", "sherpa-onnx", None, "es", &[], 16_000).unwrap_err();
        assert_eq!(run.key, "not_installed");

        let mut seen = Vec::new();
        let repair = engines.install("whisper-tiny", "sherpa-onnx", &mut |d, t| seen.push((d, t))).unwrap_err();
        assert_eq!(repair.key, "download_failed", "it downloads the model again (nothing listens here)");
        assert_eq!(seen.first(), Some(&(0, model_size)), "only the model: the engine package is whole");
        assert!(!model_dir.parent().unwrap().join(install::MARKER).exists(), "the slot is no longer marked");

        let kokoro = engines.locate("kokoro-82m-v1.0", "sherpa-onnx").unwrap();
        std::fs::remove_dir_all(&kokoro.engine_dir).unwrap();
        assert!(engines.installed().is_empty(), "no engine package root, nothing runs");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_waiting_install_reports_nothing_and_never_another_job_s_bytes() {
        let (engines, root) = mac("jobs", |_| {});
        let jobs = Jobs::default();
        jobs.set("a", 75, 100); // job A, downloading
        let held = engines.installing.lock().unwrap(); // … and holding the install lock
        std::thread::scope(|scope| {
            let b = scope.spawn(|| engines.install_job(&jobs, "b", "kokoro-82m-v1.0", "sherpa-onnx"));
            std::thread::sleep(std::time::Duration::from_millis(100));
            assert_eq!(jobs.get("b"), None, "B waits and reports nothing");
            assert_eq!(jobs.get("a"), Some([75, 100]), "A's progress is untouched");
            drop(held);
            b.join().unwrap().unwrap();
        });
        assert_eq!(jobs.get("b"), None, "B is gone once it ends");
        assert_eq!(jobs.get("a"), Some([75, 100]));

        let mut first = None;
        engines.install("whisper-tiny", "sherpa-onnx", &mut |d, t| first = first.or(Some((d, t)))).unwrap();
        assert_eq!(first, Some((0, 0)), "a job starts with (0, total); nothing left to download here");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn capabilities_are_this_device_with_its_memory() {
        let engines = NativeEngines::new(bundled_catalog(), std::env::temp_dir());
        let json = serde_json::to_value(&engines.device).unwrap();
        assert_eq!(json["runs"], "native");
        assert_eq!(json["os"], std::env::consts::OS);
        assert!(json["has"].as_array().unwrap().contains(&serde_json::json!("cpu")));
        assert!(json["memory_mb"].as_u64().is_some());
    }
}
