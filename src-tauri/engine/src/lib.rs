//! The app's side of sidevoice-engine: what the page asks through the bridge (docs/BRIDGE.md → "The native engine"),
//! in the page's terms, answered by the engine (docs/ENGINES.md).
//!
//! The engine owns the catalogue it runs, the downloads (checked against their SHA-256), the build that runs here and
//! the models themselves. This crate keeps what is the app's: the page's names for a build (catalogue model id +
//! engine id, which are the engine's model id and backend id), its install jobs (progress per job, cancel), its
//! refusals (keyed, `error.rs`), and what stays in memory and for how long (sidevoice/sidevoice-core#21 D13,
//! `residency.rs`): the engine unloads a model when the last handle to it is dropped, and the handles are kept here.
//!
//! It always runs the build the page chose — model + engine — with the accelerator the page chose or, when it names
//! none, the one the engine would use. Before it downloads or runs anything it checks, at this trust boundary, that
//! the engine runs that build here; anything else is refused with a keyed `Error`, never replaced by another build.
//!
//! The engine's futures run on the Tokio runtime whose handle `NativeEngines` is given; its own calls block on them,
//! so they are made off that runtime's worker threads (`spawn_blocking` in the app).

pub mod error;
mod jobs;
pub mod memory;
mod residency;

pub use error::Error;
pub use jobs::{Jobs, Progress};
use residency::{Calls, Key, Residency};
use sidevoice_engine::{
    Accelerator, BundledCatalog, Cancel, CatalogSource, Engine, Host, LoadedModel, Model, NativeHost,
};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// sidevoice/sidevoice-core#21 D13: how long a model stays in memory after its last use, or after the last call ended if
/// that is later. Never while a call is on.
pub const IDLE_UNLOAD: Duration = Duration::from_secs(10 * 60);

/// Mono audio in [-1, 1].
#[derive(Debug)]
pub struct Audio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

/// What a model does, as the settings window and D13 tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Task {
    Stt,
    Tts,
}

/// What `capabilities()` reports, for the page to resolve its offers from: `{runs: "native", os, arch, has,
/// memory_mb}`. `has` is the accelerators the engine can run a model on here, not every one the machine has: the page
/// offers a native build on what is in it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Device {
    pub runs: &'static str,
    pub os: String,
    pub arch: String,
    pub has: Vec<String>,
    pub memory_mb: Option<u64>,
}

/// A build on disk, as the page names it: catalogue model id + engine id.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Installed {
    pub model: String,
    pub engine: String,
}

/// What the engines take on disk, for the app's own settings window.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OnDisk {
    /// Engine packages downloaded at run time: none now, the engine's backend is linked into the app.
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
    /// What it downloaded.
    pub bytes: u64,
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

/// Faults to inject, for CI's probe build only (see `NativeEngines::faults`).
#[derive(Debug, Clone, Default)]
pub struct Faults {
    pub refuse_load: Vec<String>,
    /// (model, accelerator, delay).
    pub slow_transcribe: Vec<(String, String, Duration)>,
}

/// Whether the app offers builds that run on `accelerator`: every one the engine runs here but Core ML, which the
/// app does not offer at all.
fn offered(accelerator: Accelerator) -> bool {
    accelerator != Accelerator::CoreMl
}

/// An accelerator as the page and the catalogue name it (`cpu`, `metal`…).
pub fn accelerator_name(accelerator: Accelerator) -> String {
    match accelerator {
        Accelerator::Cpu => "cpu",
        Accelerator::Cuda => "cuda",
        Accelerator::Metal => "metal",
        Accelerator::WebGpu => "webgpu",
        Accelerator::Wasm => "wasm",
        _ => "unknown",
    }
    .to_string()
}

/// A build the page named, as the engine has it here: the engine's build of `model` on `engine` that runs here, the
/// accelerator it runs on, and what it is.
struct Located {
    model: String,
    engine: String,
    build: String,
    accelerator: String,
    task: Task,
    installed: bool,
    download_bytes: u64,
    /// The model's voices and the language each speaks (its first).
    voices: Vec<(String, Option<String>)>,
}

impl Located {
    fn key(&self) -> Key {
        (self.engine.clone(), self.model.clone(), self.accelerator.clone())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, Error> {
    mutex.lock().map_err(|_| error::internal("a lock was poisoned"))
}

/// For state every change leaves whole (one insert or removal at a time): a panic elsewhere does not spoil it.
fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn epoch_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn task_of(model: &Model) -> Task {
    if model.capabilities.contains(&sidevoice_engine::Capability::Stt) {
        Task::Stt
    } else {
        Task::Tts
    }
}

pub struct NativeEngines {
    engine: Engine,
    runtime: tokio::runtime::Handle,
    /// What `capabilities()` reports.
    pub device: Device,
    /// How long a model stays in memory unused once no call is on (D13): `IDLE_UNLOAD`.
    pub idle_unload: Duration,
    /// CI's probe build only (`src/probe.rs`): faults the room flow needs to see the interface handle — models whose
    /// load is refused, and transcriptions made slower by a delay (model, accelerator, delay). Empty in a release.
    pub faults: Faults,
    /// One install at a time, as the page's jobs expect (docs/BRIDGE.md).
    installing: Mutex<()>,
    /// One load at a time: the page's and the app's would otherwise both load a model the other is loading.
    loading: Mutex<()>,
    /// What is in memory, what D13 took out of it, and the page's unloads: one lock, so a load, an unload and a
    /// preload never interleave halfway.
    residency: Mutex<Residency<LoadedModel>>,
    calls: Mutex<Calls>,
}

impl NativeEngines {
    /// The engine with its bundled catalogue, its files in `root`, its futures on `runtime`.
    pub fn new(root: impl Into<PathBuf>, runtime: tokio::runtime::Handle) -> Result<Self, Error> {
        let host = NativeHost::new(root.into()).map_err(|e| error::internal(format!("the engine's storage: {e}")))?;
        Self::with(Box::new(host), vec![Box::new(BundledCatalog)], runtime)
    }

    /// The engine on `host`, with the catalogue `sources` make.
    pub fn with(
        host: Box<dyn Host>,
        sources: Vec<Box<dyn CatalogSource>>,
        runtime: tokio::runtime::Handle,
    ) -> Result<Self, Error> {
        let capabilities = host.capabilities();
        let engine = Engine::new(host, sources).map_err(|e| error::internal(format!("the engine: {e}")))?;
        let models = runtime.block_on(engine.models()).map_err(|e| error::internal(format!("the engine: {e}")))?;
        let mut has = Vec::new();
        for build in models.iter().flat_map(|m| &m.builds) {
            if let (true, Some(accelerator)) = (build.available, build.accelerator.filter(|a| offered(*a))) {
                let name = accelerator_name(accelerator);
                if !has.contains(&name) {
                    has.push(name);
                }
            }
        }
        let device = Device {
            runs: "native",
            os: capabilities.os,
            arch: capabilities.arch,
            has,
            memory_mb: capabilities.memory_mb.map(u64::from).or_else(memory::total_mb),
        };
        Ok(NativeEngines {
            engine,
            runtime,
            device,
            idle_unload: IDLE_UNLOAD,
            faults: Faults::default(),
            installing: Mutex::default(),
            loading: Mutex::default(),
            residency: Mutex::default(),
            calls: Mutex::default(),
        })
    }

    fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    fn models(&self) -> Result<Vec<Model>, Error> {
        self.block_on(self.engine.models()).map_err(|e| error::internal(format!("the engine's models: {e}")))
    }

    /// `model` on `engine`, if the engine can run it on this device: an engine compiled into this app, a model of its
    /// catalogue with a build for that engine, and that build runnable here (an accelerator, the model's memory).
    fn locate(&self, model_id: &str, engine_id: &str) -> Result<Located, Error> {
        if !self.engine.backends().iter().any(|backend| backend.id == engine_id) {
            return Err(error::engine_unsupported(engine_id));
        }
        let models = self.models()?;
        let model = models.iter().find(|m| m.id == model_id).ok_or_else(|| error::model_unknown(model_id))?;
        Self::located(model, engine_id)
    }

    fn located(model: &Model, engine_id: &str) -> Result<Located, Error> {
        let mut builds =
            model.builds.iter().filter(|b| b.backend == engine_id && b.accelerator.is_none_or(offered)).peekable();
        let first = *builds.peek().ok_or_else(|| error::build_missing(&model.id, engine_id))?;
        let build = builds.find(|b| b.available).unwrap_or(first);
        let Some(accelerator) = build.accelerator.filter(|_| build.available) else {
            let memory = build.reasons.iter().find(|r| r.code == "memory");
            return Err(match memory.and_then(|r| Some((r.needs?, r.has?))) {
                Some((needs, has)) => error::model_needs_memory(&model.id, needs.into(), has.into()),
                None => error::build_unfit(&model.id, engine_id, build.reasons.first().map(|r| r.code)),
            });
        };
        Ok(Located {
            model: model.id.clone(),
            engine: engine_id.to_string(),
            build: build.id.clone(),
            accelerator: accelerator_name(accelerator),
            task: task_of(model),
            installed: build.installed,
            download_bytes: build.download_bytes,
            voices: model.voices.iter().map(|v| (v.id.clone(), v.languages.first().cloned())).collect(),
        })
    }

    /// Every build the engine can run here whose files are all on disk, as the page names it.
    pub fn installed(&self) -> Result<Vec<Installed>, Error> {
        Ok(self
            .runnable()?
            .into_iter()
            .filter(|l| l.installed)
            .map(|l| Installed { model: l.model, engine: l.engine })
            .collect())
    }

    /// Every build the engine can run here, in catalogue order.
    fn runnable(&self) -> Result<Vec<Located>, Error> {
        let backends = self.engine.backends();
        let models = self.models()?;
        Ok(models.iter().flat_map(|m| backends.iter().filter_map(|b| Self::located(m, b.id).ok())).collect())
    }

    /// The model builds on disk, with the bytes each downloaded.
    pub fn on_disk(&self) -> Result<OnDisk, Error> {
        let builds = self
            .runnable()?
            .into_iter()
            .filter(|l| l.installed)
            .map(|l| BuildOnDisk {
                label: l.model.clone(),
                model: l.model,
                engine: l.engine,
                task: l.task,
                bytes: l.download_bytes,
            })
            .collect();
        Ok(OnDisk { engines: Vec::new(), builds })
    }

    /// Downloads what `model` on `engine` needs — whatever of it is missing — reporting (done, total) bytes. The first
    /// report, `(0, total)`, comes once this install holds the install lock.
    pub fn install(
        &self,
        model_id: &str,
        engine_id: &str,
        progress: &mut (dyn FnMut(u64, u64) + Send),
    ) -> Result<(), Error> {
        let progress = Mutex::new(progress);
        let held = self.install_lock(&|| false)?;
        self.install_until(&held, model_id, engine_id, &|done, total| (guard(&progress))(done, total), &|| false)
            .map(|_| ())
    }

    /// The install lock, one install at a time, waited for until `stop` says so (`install_cancelled`).
    fn install_lock(&self, stop: &dyn Fn() -> bool) -> Result<MutexGuard<'_, ()>, Error> {
        loop {
            if stop() {
                return Err(error::install_cancelled());
            }
            match self.installing.try_lock() {
                Ok(held) => return Ok(held),
                Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(50)),
                Err(TryLockError::Poisoned(_)) => return Err(error::internal("a lock was poisoned")),
            }
        }
    }

    /// `install`, under the install lock (`_held`), stopped with `install_cancelled` as soon as `stop` says so,
    /// wherever the download is, waiting on the network included (the download is dropped, and the engine keeps
    /// nothing half downloaded). Answers what it downloaded.
    fn install_until(
        &self,
        _held: &MutexGuard<'_, ()>,
        model_id: &str,
        engine_id: &str,
        progress: &(dyn Fn(u64, u64) + Sync),
        stop: &dyn Fn() -> bool,
    ) -> Result<Downloaded, Error> {
        // Asked under the lock: an install ahead of this one may have put it on disk.
        let located = self.locate(model_id, engine_id)?;
        if located.installed {
            progress(0, 0);
            return Ok(Downloaded { this: false, others: false });
        }
        let others = self.others_installed(model_id, &located.build)?;
        let total = located.download_bytes;
        progress(0, total);
        let bytes = Bytes::default();
        let sink = |report: sidevoice_engine::Progress| progress(guard(&bytes.0).add(report).min(total), total);
        let cancel = Cancel::new();
        let install = self.engine.install(model_id, Some(&located.build), &sink, &cancel);
        let stopped = async {
            while !stop() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        self.block_on(async {
            tokio::select! {
                result = install => result.map_err(|e| error::engine_failed(e, model_id, engine_id)),
                () = stopped => Err(error::install_cancelled()),
            }
        })?;
        progress(total, total);
        Ok(Downloaded { this: true, others })
    }

    /// Whether a build of `model_id` other than `build` is installed.
    fn others_installed(&self, model_id: &str, build: &str) -> Result<bool, Error> {
        let models = self.models()?;
        Ok(models
            .iter()
            .filter(|model| model.id == model_id)
            .flat_map(|model| &model.builds)
            .any(|other| other.id != build && other.installed))
    }

    /// Undoes what a cancelled job downloaded, as `undo` decides: removes the model when this job's build is all of it
    /// on disk, keeps the build when another one is installed too (the engine removes only whole models), and says
    /// which, or why removing failed. Only under the install lock (`_held`), the one the job downloaded under.
    fn roll_back(&self, _held: &MutexGuard<'_, ()>, model_id: &str, engine_id: &str, downloaded: Downloaded) -> Error {
        match undo(downloaded) {
            Undo::Nothing => error::install_cancelled(),
            Undo::Remove => match self.block_on(self.engine.uninstall(model_id)) {
                Ok(()) => error::install_cancelled(),
                Err(failed) => error::install_cancel_failed(failed, model_id, engine_id),
            },
            Undo::Keep => error::install_cancel_late(model_id, engine_id),
        }
    }

    /// `install` as the page's job `job`: known to `jobs` until it ends — its progress once it starts, cancellable
    /// throughout (`Jobs::cancel`). A cancel the job accepted wins: one that lands after the download ended removes
    /// what this job installed, so a cancelled install leaves no model to load (sidevoice-core#21 review R04) — unless
    /// another build of the model was installed before it, which removing would take too: then the build stays and the
    /// job says so (`install_cancel_late`). A removal that fails is said too (`install_cancel_failed`), never hidden.
    pub fn install_job(&self, jobs: &Jobs, job: &str, model_id: &str, engine_id: &str) -> Result<(), Error> {
        if !jobs.begin(job, model_id, engine_id) {
            return Err(error::install_cancelled());
        }
        let stop = || jobs.cancelled(job);
        // The install lock is held until the job has ended, its roll-back included: no other install can put another
        // build of the model on disk between what this job saw before downloading and the removal a late cancel makes.
        let (held, result) = match self.install_lock(&stop) {
            Ok(held) => {
                let result =
                    self.install_until(&held, model_id, engine_id, &|done, total| jobs.set(job, done, total), &stop);
                (Some(held), result)
            }
            Err(error) => (None, Err(error)),
        };
        match (result, jobs.finish(job)) {
            (result, true) => result.map(|_| ()),
            (Ok(downloaded), false) => match &held {
                Some(held) => Err(self.roll_back(held, model_id, engine_id, downloaded)),
                None => Err(error::install_cancelled()),
            },
            // Whatever the transport said after the cancel, the page asked to stop: a cancel, never a failure
            // (sidevoice-core#21 review N04).
            (Err(_), false) => Err(error::install_cancelled()),
        }
    }

    /// `model` on `engine`, on disk (and of `task`, when one is asked for), with the accelerator it runs on: the one
    /// asked for, which must be the one the engine runs this build on here, or that one.
    fn ready(
        &self,
        model_id: &str,
        engine_id: &str,
        task: Option<Task>,
        accelerator: Option<&str>,
    ) -> Result<Located, Error> {
        let located = self.locate(model_id, engine_id)?;
        if let Some(task) = task.filter(|task| *task != located.task) {
            let task = if task == Task::Stt { "speech-to-text" } else { "text-to-speech" };
            return Err(error::model_wrong_task(model_id, task));
        }
        if let Some(asked) = accelerator.filter(|asked| *asked != located.accelerator) {
            let asked = if asked.is_empty() { "unknown" } else { asked };
            return Err(error::accelerator_unusable(model_id, engine_id, asked, vec![located.accelerator.clone()]));
        }
        if !located.installed {
            return Err(error::not_installed(model_id, engine_id));
        }
        Ok(located)
    }

    fn residency(&self) -> MutexGuard<'_, Residency<LoadedModel>> {
        guard(&self.residency)
    }

    /// The located build: the one in memory, or loaded now, with the time its load took, and whether it is (still)
    /// kept in memory. One loaded now is not kept when the page unloaded it after unload number `since` — while this
    /// waited or loaded: it serves the call that loaded it, and goes.
    fn instance(&self, located: &Located, since: u64) -> Result<(LoadedModel, u64, bool), Error> {
        let key = located.key();
        if let Some((model, load_ms)) = self.residency().touch(&key) {
            return Ok((model, load_ms, true));
        }
        let _one_at_a_time = lock(&self.loading)?;
        if let Some((model, load_ms)) = self.residency().touch(&key) {
            return Ok((model, load_ms, true)); // loaded while this call waited
        }
        if self.faults.refuse_load.contains(&located.model) {
            return Err(error::runtime_failed(&located.engine, "refused by a fault the CI probe injected"));
        }
        let started = Instant::now();
        let quiet = |_: sidevoice_engine::Progress| {};
        let cancel = Cancel::new();
        let loading = self.engine.load(&located.model, Some(&located.build), &quiet, &cancel);
        let model = self.block_on(loading).map_err(|e| error::engine_failed(e, &located.model, &located.engine))?;
        let load_ms = started.elapsed().as_millis() as u64;
        let kept = self.residency().keep(key, model.clone(), located.task, load_ms, since);
        Ok((model, load_ms, kept))
    }

    /// Loads `model` on `engine` into memory, on the accelerator asked for (else the engine's): what a call runs, ready
    /// before it does. One already in memory is not loaded again; its `load_ms` is the time its load took. Refused with
    /// `load_cancelled` when the page unloads it before the load ends.
    pub fn load(&self, model_id: &str, engine_id: &str, accelerator: Option<&str>) -> Result<Load, Error> {
        // Read first: a guard held through the call would deadlock the load, which takes the lock again.
        let since = self.residency().unloads();
        self.load_since(model_id, engine_id, accelerator, since)
    }

    fn load_since(
        &self,
        model_id: &str,
        engine_id: &str,
        accelerator: Option<&str>,
        since: u64,
    ) -> Result<Load, Error> {
        let located = self.ready(model_id, engine_id, None, accelerator)?;
        let cancelled = || error::load_cancelled(model_id, engine_id, &located.accelerator);
        match self.instance(&located, since) {
            Ok((_, load_ms, true)) => Ok(Load { load_ms }),
            Ok((_, _, false)) => Err(cancelled()),
            // A load the page unloaded while it ran is cancelled, however it ended: never a failure to show (N03).
            Err(_) if self.residency().unloaded_since(&located.key(), since) => Err(cancelled()),
            Err(e) => Err(e),
        }
    }

    /// Frees `model` on `engine` on `accelerator`, or on every accelerator it is loaded on when none is named; nothing
    /// to do when it is not loaded. A call running it finishes first (it holds the model); a load of it still under
    /// way is not kept, and the app does not load it again as a call connects (D13).
    pub fn unload(&self, model_id: &str, engine_id: &str, accelerator: Option<&str>) {
        self.residency().unload(engine_id, model_id, accelerator);
    }

    /// The models in memory, oldest first.
    pub fn loaded(&self) -> Vec<Loaded> {
        self.residency().loaded()
    }

    /// This machine's memory and what is available of it now.
    pub fn memory(&self) -> Memory {
        Memory { total_mb: self.device.memory_mb, available_mb: memory::available_mb() }
    }

    /// The room says whether a call is on (joining counts). True when one has just started: the moment to `preload`.
    pub fn call_changed(&self, active: bool, now: Instant) -> bool {
        guard(&self.calls).changed(active, now)
    }

    /// D13: with no call on, frees every model unused for `idle_unload` — counted from its last use, or from when the
    /// last call ended if that is later — and remembers it for the next call. Answers what it freed.
    pub fn unload_idle(&self, now: Instant) -> Vec<Loaded> {
        let calls = guard(&self.calls);
        self.residency().unload_idle(&calls, self.idle_unload, now)
    }

    /// Loads again what `unload_idle` freed, as a call connects, each with what its load answered — unless the page
    /// has another model of that task in memory by then. One the page unloads while this runs is not kept
    /// (`load_cancelled`).
    pub fn preload(&self) -> Vec<(Loaded, Result<Load, Error>)> {
        let (evicted, since) = self.residency().take_evicted();
        evicted
            .into_iter()
            .map(|(engine, model, accelerator)| {
                let result = self.load_since(&model, &engine, Some(&accelerator), since);
                let now = epoch_ms(SystemTime::now());
                (Loaded { model, engine, accelerator, since: now, last_used: now }, result)
            })
            .collect()
    }

    /// Mono f32 samples at `sample_rate` → text, with `model` on `engine` (loaded now if it is not in memory).
    /// `language`: a BCP 47 tag, or empty to detect it.
    pub fn transcribe(
        &self,
        model_id: &str,
        engine_id: &str,
        accelerator: Option<&str>,
        language: &str,
        samples: &[f32],
        sample_rate: u32,
    ) -> Result<String, Error> {
        let located = self.ready(model_id, engine_id, Some(Task::Stt), accelerator)?;
        let since = self.residency().unloads();
        let (model, _, _) = self.instance(&located, since)?;
        let stt = model.as_stt().ok_or_else(|| error::model_wrong_task(model_id, "speech-to-text"))?;
        let language = Some(language.trim()).filter(|l| !l.is_empty());
        let text = self
            .block_on(stt.transcribe(samples, sample_rate, language))
            .map_err(|e| error::engine_failed(e, model_id, engine_id));
        let slow = self.faults.slow_transcribe.iter().find(|(m, a, _)| m == model_id && *a == located.accelerator);
        if let Some((_, _, delay)) = slow {
            std::thread::sleep(*delay);
        }
        self.residency().touch(&located.key());
        text
    }

    /// Text → mono f32 samples, with `model` on `engine` (loaded now if it is not in memory). `voice`: one of the
    /// model's voices; the language it speaks is the one its text is read in.
    pub fn synthesize(
        &self,
        model_id: &str,
        engine_id: &str,
        accelerator: Option<&str>,
        voice: &str,
        speed: f32,
        text: &str,
    ) -> Result<Audio, Error> {
        let located = self.ready(model_id, engine_id, Some(Task::Tts), accelerator)?;
        let language = located.voices.iter().find(|(id, _)| id == voice).map(|(_, language)| language.clone());
        let language = language.ok_or_else(|| error::voice_unknown(model_id, voice))?;
        let since = self.residency().unloads();
        let (model, _, _) = self.instance(&located, since)?;
        let tts = model.as_tts().ok_or_else(|| error::model_wrong_task(model_id, "text-to-speech"))?;
        let speed = Some(speed).filter(|s| *s > 0.0);
        let audio = self
            .block_on(tts.speak(text, voice, language.as_deref(), speed))
            .map_err(|e| error::engine_failed(e, model_id, engine_id));
        self.residency().touch(&located.key());
        audio.map(|audio| Audio { samples: audio.samples, sample_rate: audio.sample_rate })
    }
}

/// What an install put on disk: whether it downloaded the build (`this`), and whether another build of the model was
/// installed already (`others`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Downloaded {
    this: bool,
    others: bool,
}

/// What a cancel that landed after the download ended does with it.
#[derive(Debug, PartialEq)]
enum Undo {
    /// Nothing was downloaded.
    Nothing,
    /// The build is all of the model on disk: removing the model removes only it.
    Remove,
    /// Another build is installed: removing the model would take it too, so the build stays.
    Keep,
}

fn undo(downloaded: Downloaded) -> Undo {
    match downloaded {
        Downloaded { this: false, .. } => Undo::Nothing,
        Downloaded { others: false, .. } => Undo::Remove,
        Downloaded { others: true, .. } => Undo::Keep,
    }
}

/// The bytes an install has downloaded so far, from the engine's reports: the files it finished, and the one it is
/// downloading.
#[derive(Default)]
struct Bytes(Mutex<BytesState>);

#[derive(Default)]
struct BytesState {
    finished: u64,
    files_done: usize,
    current: u64,
}

impl BytesState {
    fn add(&mut self, report: sidevoice_engine::Progress) -> u64 {
        if report.done > self.files_done {
            self.files_done = report.done;
            self.finished += self.current;
            self.current = 0;
        }
        if report.received > 0 {
            self.current = report.received;
        }
        self.finished + self.current
    }
}

#[cfg(test)]
mod tests;
