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
//!
//! A model in memory is one per (engine, model, accelerator); the language is each call's. The page loads and unloads
//! them (`load`, `unload`), and running one loads it if it is not. The app keeps them by rubasace/sidevoice#124 D13:
//! loaded while a call is on and for `idle_unload` after the last use once no call is (`unload_idle`); a model it
//! unloaded that way is loaded again as the next call connects (`call_changed`, `preload`).

pub mod error;
pub mod install;
pub mod memory;
pub mod sherpa;
pub mod sherpa_ffi;

pub use error::Error;
use install::Store;
use sidevoice_desktop_core::engines::{self, Build, Capability, Catalog, Device, Download, Engine, Model, Package};
use sidevoice_desktop_core::engines::{Runs, Task};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// rubasace/sidevoice#124 D13: how long a model stays in memory after its last use, or after the last call ended if
/// that is later. Never while a call is on.
pub const IDLE_UNLOAD: Duration = Duration::from_secs(10 * 60);

/// Mono audio in [-1, 1].
pub struct Audio {
    pub samples: Vec<f32>,
    pub sample_rate: i32,
}

/// A native engine as this app runs it, loaded from its unpacked package: it turns a model build (its files in
/// `dir`, the engine's `config` from the catalogue) into something that runs, on the accelerator it is given.
pub trait Runtime: Send + Sync {
    /// Speech to text, loaded into memory; each transcription names its language.
    fn recognizer(
        &self,
        family: &str,
        dir: &Path,
        config: &serde_json::Value,
        accelerator: Capability,
    ) -> Result<Box<dyn Recognize>, Error>;

    /// Text to speech, loaded into memory. `language`: the phonemizer's until a synthesis names its own (the model's
    /// first voice's).
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
    /// Mono samples at `sample_rate` → text. `language`: the model family's code, or empty to detect it.
    fn transcribe(&self, samples: &[f32], sample_rate: i32, language: &str) -> Result<String, Error>;
}

pub trait Speak: Send + Sync {
    /// Text → audio, with the model's speaker `sid`. `language`: the voice's, for the phonemizer.
    fn synthesize(&self, text: &str, sid: i32, speed: f32, language: &str) -> Result<Audio, Error>;
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
pub const ADAPTERS: &[Adapter] = &[Adapter { engine: "sherpa-onnx", load: sherpa::load, files: sherpa::files }];

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

/// A model in memory: (engine, model, accelerator).
type Key = (String, String, Capability);

#[derive(Clone)]
enum Instance {
    Stt(Arc<dyn Recognize>),
    Tts(Arc<dyn Speak>),
}

struct Resident {
    instance: Instance,
    task: Task,
    load_ms: u64,
    since: SystemTime,
    /// The last use, for the idle rule (monotonic) and for the page (wall clock).
    used: Instant,
    used_at: SystemTime,
}

/// A model in memory, as `loaded()` reports it; `since` and `last_used` in milliseconds since the Unix epoch.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Loaded {
    pub model: String,
    pub engine: String,
    pub accelerator: String,
    pub since: u64,
    pub last_used: u64,
}

/// What `load` answers: how long loading the model took.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Load {
    pub load_ms: u64,
}

/// What `memory()` answers: the machine's memory and what is available of it now, in MiB, `None` when unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Memory {
    pub total_mb: Option<u64>,
    pub available_mb: Option<u64>,
}

/// Whether a call is on, as the room reports it, and when the last one ended.
#[derive(Default)]
struct Calls {
    active: bool,
    ended: Option<Instant>,
}

pub struct NativeEngines {
    pub catalog: Catalog,
    /// What `capabilities()` reports: this process's OS, architecture and accelerators, and the OS's memory.
    pub device: Device,
    /// How long a model stays in memory unused once no call is on (D13): `IDLE_UNLOAD`.
    pub idle_unload: Duration,
    store: Store,
    adapters: Vec<Adapter>,
    /// One loaded runtime per engine.
    runtimes: Mutex<HashMap<String, Arc<dyn Runtime>>>,
    /// One install at a time: two would download into the same staging paths and break each other.
    installing: Mutex<()>,
    /// One load at a time: the page's and the app's would otherwise load the same model twice.
    loading: Mutex<()>,
    resident: Mutex<HashMap<Key, Resident>>,
    /// What `unload_idle` took out of memory (the newest of each task), to load again as the next call connects unless
    /// the page has unloaded it, or has another model of that task in memory by then.
    evicted: Mutex<Vec<(Key, Task)>>,
    calls: Mutex<Calls>,
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

/// For state every change leaves whole (one insert or removal at a time): a panic elsewhere does not spoil it.
fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn epoch_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl NativeEngines {
    pub fn new(catalog: Catalog, root: impl Into<PathBuf>) -> Self {
        let mut device = engines::native_device();
        device.memory_mb = memory::total_mb();
        NativeEngines {
            catalog,
            device,
            idle_unload: IDLE_UNLOAD,
            store: Store::new(root),
            adapters: ADAPTERS.to_vec(),
            runtimes: Mutex::default(),
            installing: Mutex::default(),
            loading: Mutex::default(),
            resident: Mutex::default(),
            evicted: Mutex::default(),
            calls: Mutex::default(),
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

    /// `model` on `engine`, downloaded (and of `task`, when one is asked for), with the accelerator it runs on: the
    /// one asked for, which must be one this build can use here, or the first it can.
    fn ready(
        &self,
        model_id: &str,
        engine_id: &str,
        task: Option<Task>,
        accelerator: Option<Capability>,
    ) -> Result<(Located<'_>, Capability), Error> {
        let located = self.locate(model_id, engine_id)?;
        if let Some(task) = task.filter(|task| self.catalog.task_of(located.model) != Some(*task)) {
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

    /// The model in memory under `key`, marked used now, with the time its load took.
    fn touch(&self, key: &Key) -> Option<(Instance, u64)> {
        let mut resident = guard(&self.resident);
        let found = resident.get_mut(key)?;
        (found.used, found.used_at) = (Instant::now(), SystemTime::now());
        Some((found.instance.clone(), found.load_ms))
    }

    /// The located build in memory on `accelerator`, loaded now if it is not, with the time its load took.
    fn instance(&self, located: &Located, accelerator: Capability) -> Result<(Instance, u64), Error> {
        let key = (located.engine.id.clone(), located.model.id.clone(), accelerator);
        if let Some(found) = self.touch(&key) {
            return Ok(found);
        }
        let _one_at_a_time = lock(&self.loading)?;
        if let Some(found) = self.touch(&key) {
            return Ok(found); // loaded while this call waited
        }
        let (family, dir, config) = (&located.model.family, &located.model_dir, &located.build.config);
        let task =
            self.catalog.task_of(located.model).ok_or_else(|| error::family_unsupported(&located.engine.id, family))?;
        let started = Instant::now();
        let runtime = self.runtime(located)?;
        let instance = match task {
            Task::Stt => Instance::Stt(Arc::from(runtime.recognizer(family, dir, config, accelerator)?)),
            Task::Tts => {
                let language = located.model.voices.first().map_or("", |v| v.language.as_str());
                Instance::Tts(Arc::from(runtime.voice(family, dir, config, language, accelerator)?))
            }
        };
        let load_ms = started.elapsed().as_millis() as u64;
        let (since, used) = (SystemTime::now(), Instant::now());
        let resident = Resident { instance: instance.clone(), task, load_ms, since, used, used_at: since };
        guard(&self.resident).insert(key, resident);
        Ok((instance, load_ms))
    }

    /// Loads `model` on `engine` into memory, on the accelerator asked for (else the first it can use here): what a
    /// call runs, ready before it does. One already in memory is not loaded again; its `load_ms` is the time its
    /// load took.
    pub fn load(&self, model_id: &str, engine_id: &str, accelerator: Option<Capability>) -> Result<Load, Error> {
        let (located, accelerator) = self.ready(model_id, engine_id, None, accelerator)?;
        let (_, load_ms) = self.instance(&located, accelerator)?;
        Ok(Load { load_ms })
    }

    /// Frees `model` on `engine`, on whatever accelerators it is loaded; nothing to do when it is not. A call running
    /// it finishes first.
    pub fn unload(&self, model_id: &str, engine_id: &str) {
        let ours = |(engine, model, _): &Key| engine == engine_id && model == model_id;
        guard(&self.resident).retain(|key, _| !ours(key));
        // The page unloads what it no longer uses: not loaded again for it as a call connects.
        guard(&self.evicted).retain(|(key, _)| !ours(key));
    }

    /// The models in memory, oldest first.
    pub fn loaded(&self) -> Vec<Loaded> {
        let resident = guard(&self.resident);
        let mut loaded: Vec<(SystemTime, Loaded)> = resident
            .iter()
            .map(|((engine, model, accelerator), r)| {
                let loaded = Loaded {
                    model: model.clone(),
                    engine: engine.clone(),
                    accelerator: engines::capability_name(*accelerator),
                    since: epoch_ms(r.since),
                    last_used: epoch_ms(r.used_at),
                };
                (r.since, loaded)
            })
            .collect();
        loaded.sort_by(|a, b| {
            a.0.cmp(&b.0).then_with(|| (&a.1.model, &a.1.accelerator).cmp(&(&b.1.model, &b.1.accelerator)))
        });
        loaded.into_iter().map(|(_, l)| l).collect()
    }

    /// This machine's memory and what is available of it now.
    pub fn memory(&self) -> Memory {
        Memory { total_mb: self.device.memory_mb, available_mb: memory::available_mb() }
    }

    /// The room says whether a call is on (joining counts). True when one has just started: the moment to `preload`.
    pub fn call_changed(&self, active: bool, now: Instant) -> bool {
        let mut calls = guard(&self.calls);
        let started = active && !calls.active;
        if calls.active && !active {
            calls.ended = Some(now);
        }
        calls.active = active;
        started
    }

    /// D13: with no call on, frees every model unused for `idle_unload` — counted from its last use, or from when the
    /// last call ended if that is later — and remembers it for the next call. Answers what it freed.
    pub fn unload_idle(&self, now: Instant) -> Vec<Loaded> {
        let calls = guard(&self.calls);
        if calls.active {
            return Vec::new();
        }
        let idle = |r: &Resident| {
            let from = calls.ended.map_or(r.used, |ended| ended.max(r.used));
            now.saturating_duration_since(from) >= self.idle_unload
        };
        let mut resident = guard(&self.resident);
        let mut keys: Vec<(SystemTime, Key)> =
            resident.iter().filter(|(_, r)| idle(r)).map(|(k, r)| (r.since, k.clone())).collect();
        keys.sort_by_key(|k| k.0); // oldest first: of two of one task, the newer is the one remembered
        let keys = keys.into_iter().map(|(_, k)| k);
        let mut freed = Vec::new();
        for key in keys {
            let Some(r) = resident.remove(&key) else { continue };
            let (engine, model, accelerator) = key.clone();
            let accelerator_name = engines::capability_name(accelerator);
            let since = epoch_ms(r.since);
            freed.push(Loaded { model, engine, accelerator: accelerator_name, since, last_used: epoch_ms(r.used_at) });
            let mut evicted = guard(&self.evicted);
            evicted.retain(|(k, task)| *k != key && *task != r.task);
            evicted.push((key, r.task));
        }
        freed
    }

    /// Loads again what `unload_idle` freed, as a call connects, each with what its load answered — unless the page
    /// has another model of that task in memory by then. One it loaded to check while this stayed its choice, and
    /// unloaded when the check failed (#124 D11), does not count: this one comes back.
    pub fn preload(&self) -> Vec<(Loaded, Result<Load, Error>)> {
        let evicted = std::mem::take(&mut *guard(&self.evicted));
        let replaced: Vec<Task> = guard(&self.resident).values().map(|r| r.task).collect();
        evicted
            .into_iter()
            .filter(|(_, task)| !replaced.contains(task))
            .map(|((engine, model, accelerator), _)| {
                let result = self.load(&model, &engine, Some(accelerator));
                let now = epoch_ms(SystemTime::now());
                let accelerator = engines::capability_name(accelerator);
                (Loaded { model, engine, accelerator, since: now, last_used: now }, result)
            })
            .collect()
    }

    /// Mono f32 samples at `sample_rate` → text, with `model` on `engine` (loaded now if it is not in memory).
    /// `language`: the family's code, or empty to detect it.
    pub fn transcribe(
        &self,
        model_id: &str,
        engine_id: &str,
        accelerator: Option<Capability>,
        language: &str,
        samples: &[f32],
        sample_rate: i32,
    ) -> Result<String, Error> {
        let (located, accelerator) = self.ready(model_id, engine_id, Some(Task::Stt), accelerator)?;
        let Instance::Stt(recognizer) = self.instance(&located, accelerator)?.0 else {
            return Err(error::internal("a voice model loaded for transcription"));
        };
        let text = recognizer.transcribe(samples, sample_rate, language.trim());
        self.touch(&(engine_id.to_string(), model_id.to_string(), accelerator));
        text
    }

    /// Text → mono f32 samples, with `model` on `engine` (loaded now if it is not in memory). `voice`: a voice id the
    /// catalogue lists for the model (its language picks the phonemizer's).
    pub fn synthesize(
        &self,
        model_id: &str,
        engine_id: &str,
        accelerator: Option<Capability>,
        voice: &str,
        speed: f32,
        text: &str,
    ) -> Result<Audio, Error> {
        let (located, accelerator) = self.ready(model_id, engine_id, Some(Task::Tts), accelerator)?;
        let chosen = located.model.voices.iter().find(|v| v.id == voice);
        let chosen = chosen.ok_or_else(|| error::voice_unknown(model_id, voice))?;
        let Instance::Tts(speaker) = self.instance(&located, accelerator)?.0 else {
            return Err(error::internal("a transcription model loaded for speech"));
        };
        let audio = speaker.synthesize(text, chosen.sid, if speed > 0.0 { speed } else { 1.0 }, &chosen.language);
        self.touch(&(engine_id.to_string(), model_id.to_string(), accelerator));
        audio
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

    /// What the fake runtime was asked to load, in order, by every test: (model directory, "family model
    /// accelerator").
    static LOADED: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());
    /// What the fake voices said, by every test: (model directory, "language text").
    static SPOKEN: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());

    struct Fake;
    struct Heard(String);
    struct Said(PathBuf);

    /// A model the fake runtime cannot load: what a build that does not load on this machine looks like.
    const BROKEN: &str = "whisper-base";

    impl Runtime for Fake {
        fn recognizer(
            &self,
            family: &str,
            dir: &Path,
            _config: &serde_json::Value,
            accelerator: Capability,
        ) -> Result<Box<dyn Recognize>, Error> {
            if leaf(dir) == BROKEN {
                return Err(error::runtime_failed("sherpa-onnx", "this model does not load here"));
            }
            let what = format!("{family} {} {}", leaf(dir), engines::capability_name(accelerator));
            LOADED.lock().unwrap().push((dir.to_path_buf(), what.clone()));
            Ok(Box::new(Heard(what)))
        }

        fn voice(
            &self,
            family: &str,
            dir: &Path,
            _config: &serde_json::Value,
            _language: &str,
            accelerator: Capability,
        ) -> Result<Box<dyn Speak>, Error> {
            LOADED
                .lock()
                .unwrap()
                .push((dir.to_path_buf(), format!("{family} {} {}", leaf(dir), engines::capability_name(accelerator))));
            Ok(Box::new(Said(dir.to_path_buf())))
        }
    }

    impl Recognize for Heard {
        fn transcribe(&self, _samples: &[f32], _rate: i32, language: &str) -> Result<String, Error> {
            Ok(format!("{} {language}", self.0))
        }
    }

    impl Speak for Said {
        fn synthesize(&self, text: &str, sid: i32, _speed: f32, language: &str) -> Result<Audio, Error> {
            SPOKEN.lock().unwrap().push((self.0.clone(), format!("{language} {text}")));
            Ok(Audio { samples: vec![0.0; text.len()], sample_rate: sid })
        }
    }

    /// The entries of `log` made under `root` (each test has its own).
    fn ours(log: &Mutex<Vec<(PathBuf, String)>>, root: &Path) -> Vec<String> {
        log.lock().unwrap().iter().filter(|(dir, _)| dir.starts_with(root)).map(|(_, l)| l.clone()).collect()
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
    fn runs_the_build_and_accelerator_it_is_asked_for_one_model_in_memory_for_every_language() {
        let (engines, root) = mac("choice", |_| {});
        let heard = engines.transcribe("whisper-tiny", "sherpa-onnx", None, "es", &[0.0], 16_000).unwrap();
        assert_eq!(heard, "whisper whisper-tiny cpu es", "no accelerator named: the first the build can use here");
        let heard = engines.transcribe("whisper-tiny", "sherpa-onnx", Some(Capability::Coreml), "es", &[], 16_000);
        assert_eq!(heard.unwrap(), "whisper whisper-tiny coreml es", "the one asked for");
        let heard = engines.transcribe("whisper-tiny", "sherpa-onnx", None, " en ", &[], 16_000).unwrap();
        assert_eq!(heard, "whisper whisper-tiny cpu en", "another language, the same model in memory");
        let metal = engines.transcribe("whisper-tiny", "sherpa-onnx", Some(Capability::Metal), "es", &[], 16_000);
        let metal = metal.unwrap_err();
        assert_eq!(metal.key, "accelerator_unusable");
        assert_eq!(metal.params["accelerator"], "metal");
        assert_eq!(metal.params["usable"], serde_json::json!(["cpu", "coreml"]));
        let audio = engines.synthesize("kokoro-82m-v1.0", "sherpa-onnx", None, "ef_dora", 1.0, "hola").unwrap();
        assert_eq!((audio.samples.len(), audio.sample_rate), (4, 28), "ef_dora's speaker");
        engines.synthesize("kokoro-82m-v1.0", "sherpa-onnx", None, "af_heart", 1.0, "hello").unwrap();
        assert_eq!(ours(&SPOKEN, &root), ["es hola", "en-us hello"], "each voice's language, per call");
        assert_eq!(
            ours(&LOADED, &root),
            ["whisper whisper-tiny cpu", "whisper whisper-tiny coreml", "kokoro kokoro-82m-v1.0 cpu"],
            "each (model, accelerator) loaded once"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn load_puts_a_model_in_memory_once_loaded_lists_it_and_unload_frees_it() {
        let (engines, root) = mac("load", |_| {});
        assert!(engines.loaded().is_empty(), "nothing is loaded before it is asked for");
        let first = engines.load("whisper-tiny", "sherpa-onnx", Some(Capability::Cpu)).unwrap();
        let again = engines.load("whisper-tiny", "sherpa-onnx", None).unwrap();
        assert_eq!(again, first, "already in memory (the resolver's accelerator is cpu): its load time, not a reload");
        engines.transcribe("whisper-tiny", "sherpa-onnx", None, "es", &[], 16_000).unwrap();
        engines.load("kokoro-82m-v1.0", "sherpa-onnx", None).unwrap();
        assert_eq!(ours(&LOADED, &root), ["whisper whisper-tiny cpu", "kokoro kokoro-82m-v1.0 cpu"]);

        let loaded = engines.loaded();
        let names: Vec<String> = loaded.iter().map(|l| format!("{}@{}/{}", l.model, l.engine, l.accelerator)).collect();
        assert_eq!(names, ["whisper-tiny@sherpa-onnx/cpu", "kokoro-82m-v1.0@sherpa-onnx/cpu"], "oldest first");
        let now = epoch_ms(SystemTime::now());
        assert!(loaded.iter().all(|l| l.since <= l.last_used && l.last_used <= now && now - l.since < 60_000));
        let json = serde_json::to_value(&loaded[0]).unwrap();
        let fields: Vec<&String> = json.as_object().unwrap().keys().collect();
        assert_eq!(fields, ["accelerator", "engine", "last_used", "model", "since"]);

        engines.unload("whisper-tiny", "sherpa-onnx");
        engines.unload("whisper-tiny", "sherpa-onnx"); // nothing left to free: not an error
        engines.unload("whisper-nope", "sherpa-onnx");
        assert_eq!(engines.loaded().iter().map(|l| l.model.as_str()).collect::<Vec<_>>(), ["kokoro-82m-v1.0"]);
        engines.transcribe("whisper-tiny", "sherpa-onnx", None, "es", &[], 16_000).unwrap();
        assert_eq!(ours(&LOADED, &root).len(), 3, "running an unloaded model loads it again");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_load_that_fails_is_refused_by_key_and_leaves_what_is_loaded_as_it_was() {
        let (engines, root) = mac("fail", |_| {});
        let base = engines.locate(BROKEN, "sherpa-onnx").unwrap();
        mark(&base.model_dir, base.model_download, &base.model_files);
        engines.load("whisper-tiny", "sherpa-onnx", None).unwrap();
        let failed = engines.load(BROKEN, "sherpa-onnx", None).unwrap_err();
        assert_eq!((failed.key, failed.params["engine"].clone()), ("runtime_failed", "sherpa-onnx".into()));
        assert_eq!(engines.load("whisper-small", "sherpa-onnx", None).unwrap_err().key, "not_installed");
        assert_eq!(
            engines.load("whisper-tiny", "sherpa-onnx", Some(Capability::Metal)).unwrap_err().key,
            "accelerator_unusable"
        );
        assert_eq!(engines.load("whisper-tiny", "transformers-js", None).unwrap_err().key, "engine_unsupported");
        let loaded: Vec<String> = engines.loaded().into_iter().map(|l| l.model).collect();
        assert_eq!(loaded, ["whisper-tiny"], "the previous model is still in memory, nothing half-loaded beside it");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn d13_no_unload_during_a_call_then_ten_minutes_after_the_last_use_or_the_call_s_end() {
        let (engines, root) = mac("idle", |_| {});
        assert_eq!(engines.idle_unload, Duration::from_secs(600));
        // No call yet: ten minutes after its last use.
        engines.load("whisper-tiny", "sherpa-onnx", None).unwrap();
        let used = Instant::now();
        assert!(engines.unload_idle(used + Duration::from_secs(9 * 60)).is_empty());
        assert_eq!(engines.unload_idle(used + Duration::from_secs(10 * 60 + 1)).len(), 1);

        let start = Instant::now();
        let minutes = |m: u64| start + Duration::from_secs(m * 60);
        engines.load("whisper-tiny", "sherpa-onnx", None).unwrap();
        engines.load("kokoro-82m-v1.0", "sherpa-onnx", None).unwrap();
        assert!(engines.call_changed(true, minutes(1)), "a call connects");
        assert!(!engines.call_changed(true, minutes(2)), "still the same call");
        assert!(engines.unload_idle(minutes(45)).is_empty(), "never during a call, however long since the last use");
        assert!(!engines.call_changed(false, minutes(50)), "the last one leaves");
        assert!(engines.unload_idle(minutes(59)).is_empty(), "nine minutes after the call ended");
        let freed: Vec<String> =
            engines.unload_idle(minutes(60) + Duration::from_secs(1)).into_iter().map(|l| l.model).collect();
        assert_eq!(freed, ["whisper-tiny", "kokoro-82m-v1.0"], "ten minutes after the call ended, oldest first");
        assert!(engines.loaded().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn what_an_idle_unload_freed_is_loaded_again_as_the_next_call_connects_unless_the_page_moved_on() {
        let (engines, root) = mac("preload", |_| {});
        let base = engines.locate(BROKEN, "sherpa-onnx").unwrap();
        mark(&base.model_dir, base.model_download, &base.model_files);
        let small = engines.locate("whisper-small", "sherpa-onnx").unwrap();
        mark(&small.model_dir, small.model_download, &small.model_files);
        let later = || Instant::now() + Duration::from_secs(11 * 60);
        let models = |engines: &NativeEngines| -> Vec<String> {
            engines.loaded().into_iter().map(|l| format!("{}/{}", l.model, l.accelerator)).collect()
        };
        engines.load("whisper-tiny", "sherpa-onnx", Some(Capability::Coreml)).unwrap();
        engines.load("kokoro-82m-v1.0", "sherpa-onnx", None).unwrap();
        assert_eq!(engines.unload_idle(later()).len(), 2);
        assert!(engines.call_changed(true, Instant::now()));
        let preloaded = engines.preload();
        let names: Vec<String> =
            preloaded.iter().map(|(l, r)| format!("{}/{} {}", l.model, l.accelerator, r.is_ok())).collect();
        assert_eq!(names, ["whisper-tiny/coreml true", "kokoro-82m-v1.0/cpu true"], "on the accelerator it had");
        assert_eq!(models(&engines), ["whisper-tiny/coreml", "kokoro-82m-v1.0/cpu"]);
        assert!(engines.preload().is_empty(), "loaded again once");

        // The page tries another transcription model, which fails to load: its choice is still whisper-tiny.
        engines.call_changed(false, Instant::now());
        assert_eq!(engines.unload_idle(later()).len(), 2);
        assert!(engines.load(BROKEN, "sherpa-onnx", None).is_err());
        engines.unload(BROKEN, "sherpa-onnx");
        engines.call_changed(true, Instant::now());
        assert_eq!(engines.preload().len(), 2);
        assert_eq!(models(&engines), ["whisper-tiny/coreml", "kokoro-82m-v1.0/cpu"]);

        // It unloads its voice, and swaps whisper-tiny for whisper-small: neither freed one comes back.
        engines.call_changed(false, Instant::now());
        assert_eq!(engines.unload_idle(later()).len(), 2);
        engines.unload("kokoro-82m-v1.0", "sherpa-onnx");
        engines.load("whisper-small", "sherpa-onnx", None).unwrap();
        engines.call_changed(true, Instant::now());
        assert!(engines.preload().is_empty());
        assert_eq!(models(&engines), ["whisper-small/cpu"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn memory_is_the_machine_s_total_and_what_is_available_now() {
        let engines = NativeEngines::new(bundled_catalog(), std::env::temp_dir());
        let memory = serde_json::to_value(engines.memory()).unwrap();
        let (total, available) = (memory["total_mb"].as_u64().unwrap(), memory["available_mb"].as_u64().unwrap());
        assert_eq!(Some(total), engines.device.memory_mb);
        assert!(available <= total);
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
