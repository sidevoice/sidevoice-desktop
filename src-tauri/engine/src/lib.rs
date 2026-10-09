//! The app's side of sidevoice-engine: what the page asks through the bridge (docs/BRIDGE.md → "The native engine"),
//! in the page's terms, answered by the engine (docs/ENGINES.md), and the engine itself, to load and run its models.
//!
//! The engine owns the catalogue it runs, the downloads (checked against their SHA-256), the build that runs here and
//! the models themselves. This crate keeps what is the app's: the page's names for a build (catalogue model id +
//! engine id, which are the engine's model id and backend id), its install jobs (progress per job, cancel) and its
//! refusals (keyed, `error.rs`). Loading and running the models goes through the
//! engine itself (`NativeEngines::engine`): what is in memory is what its `LoadedModel`s hold.
//!
//! It always installs the build the page chose — model + engine. Before it downloads anything it checks, at this trust
//! boundary, that the engine runs that build here; anything else is refused with a keyed `Error`, never replaced by
//! another build.
//!
//! The engine's futures run on the Tokio runtime whose handle `NativeEngines` is given; its own calls block on them,
//! so they are made off that runtime's worker threads (`spawn_blocking` in the app).

pub mod error;
mod jobs;
pub mod memory;

pub use error::Error;
pub use jobs::{Jobs, Progress};
/// The engine crate itself, for what the app hands it (`Credentials`) and the models it loads.
pub use sidevoice_engine;
use sidevoice_engine::{
    Accelerator, BundledCatalog, Cancel, CatalogSource, Credentials, Engine, Host, Model, NativeHost,
};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::Duration;

/// What a model does, as the settings window tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Task {
    Stt,
    Tts,
    Vad,
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

/// What `memory()` answers: the machine's memory and what is available of it now, in MiB, `None` when unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Memory {
    pub total_mb: Option<u64>,
    pub available_mb: Option<u64>,
}

/// Whether the app offers builds that run on `accelerator`: every one the engine runs here but Core ML, which the
/// app does not offer at all.
fn offered(accelerator: Accelerator) -> bool {
    accelerator != Accelerator::CoreMl
}

/// An accelerator as the page and the catalogue name it (`cpu`, `metal`, `remote`…).
pub fn accelerator_name(accelerator: Accelerator) -> String {
    match accelerator {
        Accelerator::Cpu => "cpu",
        Accelerator::Cuda => "cuda",
        Accelerator::Metal => "metal",
        Accelerator::WebGpu => "webgpu",
        Accelerator::Wasm => "wasm",
        Accelerator::Remote => "remote",
        _ => "unknown",
    }
    .to_string()
}

/// A build the page named, as the engine has it here: the engine's build of `model` on `engine` that runs here, and
/// what it is.
struct Located {
    model: String,
    engine: String,
    build: String,
    task: Task,
    installed: bool,
    download_bytes: u64,
}

/// For state every change leaves whole (one insert or removal at a time): a panic elsewhere does not spoil it.
fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn task_of(model: &Model) -> Task {
    let can = |capability| model.capabilities.contains(&capability);
    if can(sidevoice_engine::Capability::Stt) {
        Task::Stt
    } else if can(sidevoice_engine::Capability::Vad) {
        Task::Vad
    } else {
        Task::Tts
    }
}

pub struct NativeEngines {
    engine: Arc<Engine>,
    runtime: tokio::runtime::Handle,
    /// What `capabilities()` reports.
    pub device: Device,
    /// One install at a time, as the page's jobs expect (docs/BRIDGE.md).
    installing: Mutex<()>,
}

impl NativeEngines {
    /// The engine with its bundled catalogue, its files in `root`, the keys of remote providers from `credentials`,
    /// its futures on `runtime`.
    pub fn new(
        root: impl Into<PathBuf>,
        credentials: impl Credentials + 'static,
        runtime: tokio::runtime::Handle,
    ) -> Result<Self, Error> {
        let host = NativeHost::new(root.into()).map_err(|e| error::internal(format!("the engine's storage: {e}")))?;
        Self::with(Box::new(host.with_credentials(credentials)), vec![Box::new(BundledCatalog)], runtime)
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
        Ok(NativeEngines { engine: Arc::new(engine), runtime, device, installing: Mutex::default() })
    }

    /// The engine itself, to load and run its models.
    pub fn engine(&self) -> Arc<Engine> {
        Arc::clone(&self.engine)
    }

    fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    /// Every model of the catalogue, with its builds ranked here.
    pub fn models(&self) -> Result<Vec<Model>, Error> {
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
        if !build.available || build.accelerator.is_none() {
            let memory = build.reasons.iter().find(|r| r.code == "memory");
            return Err(match memory.and_then(|r| Some((r.needs?, r.has?))) {
                Some((needs, has)) => error::model_needs_memory(&model.id, needs.into(), has.into()),
                None => error::build_unfit(&model.id, engine_id, build.reasons.first().map(|r| r.code)),
            });
        }
        Ok(Located {
            model: model.id.clone(),
            engine: engine_id.to_string(),
            build: build.id.clone(),
            task: task_of(model),
            installed: build.installed,
            download_bytes: build.download_bytes,
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
        self.install_until(model_id, engine_id, &|done, total| (guard(&progress))(done, total), &|| false).map(|_| ())
    }

    /// `install`, stopped with `install_cancelled` as soon as `stop` says so: while it waits for another install, or
    /// wherever the download is, waiting on the network included (the download is dropped, and the engine keeps
    /// nothing half downloaded). Answers whether it downloaded the build.
    fn install_until(
        &self,
        model_id: &str,
        engine_id: &str,
        progress: &(dyn Fn(u64, u64) + Sync),
        stop: &dyn Fn() -> bool,
    ) -> Result<bool, Error> {
        let located = self.locate(model_id, engine_id)?;
        let _one_at_a_time = loop {
            if stop() {
                return Err(error::install_cancelled());
            }
            match self.installing.try_lock() {
                Ok(held) => break held,
                Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(50)),
                Err(TryLockError::Poisoned(_)) => return Err(error::internal("a lock was poisoned")),
            }
        };
        // Asked again under the lock: an install ahead of this one may have put it on disk.
        let located = self.locate(model_id, engine_id).unwrap_or(located);
        if located.installed {
            progress(0, 0);
            return Ok(false);
        }
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
        Ok(true)
    }

    /// `install` as the page's job `job`: known to `jobs` until it ends — its progress once it starts, cancellable
    /// throughout (`Jobs::cancel`). A cancel the job accepted always wins: one that lands after the download ended
    /// removes the model this job installed, so a cancelled install never leaves a model to load (sidevoice-core#21
    /// review R04).
    pub fn install_job(&self, jobs: &Jobs, job: &str, model_id: &str, engine_id: &str) -> Result<(), Error> {
        if !jobs.begin(job, model_id, engine_id) {
            return Err(error::install_cancelled());
        }
        let result =
            self.install_until(model_id, engine_id, &|done, total| jobs.set(job, done, total), &|| jobs.cancelled(job));
        match (result, jobs.finish(job)) {
            (result, true) => result.map(|_| ()),
            (Ok(downloaded), false) => {
                if downloaded {
                    let _ = self.block_on(self.engine.uninstall(model_id));
                }
                Err(error::install_cancelled())
            }
            // Whatever the transport said after the cancel, the page asked to stop: a cancel, never a failure
            // (sidevoice-core#21 review N04).
            (Err(_), false) => Err(error::install_cancelled()),
        }
    }

    /// This machine's memory and what is available of it now.
    pub fn memory(&self) -> Memory {
        Memory { total_mb: self.device.memory_mb, available_mb: memory::available_mb() }
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
