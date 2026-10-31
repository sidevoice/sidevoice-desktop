//! The application, as a shell sees it: one façade, [`AppHost`], over the voice call, the engine, the audio devices and
//! the lifecycle. A shell (Tauri today) only turns its transport into calls of it, and its events and admissions into
//! the three hooks it implements; what the application does with them is here, and runs without any shell.
//!
//! - **Admission.** A change of the app's data (the desktop's reset) must not overlap work that uses it. Work that
//!   does asks [`Admission`] for a [`Lease`] and holds it, as an RAII value, across everything it awaits and every
//!   blocking thread it hands work to; the app that never resets admits every time ([`AlwaysAdmit`]).
//! - **Runtime.** The Tokio runtime is a handle the shell passes in, never an ambient one: every command runs with it
//!   entered, so whatever the call's lanes, the device watch and the engine's futures spawn lands on it, whichever executor
//!   the caller polls the command on, and its blocking pool takes the models' native work. What loads models
//!   ([`AppHost::start`], [`AppHost::update`], [`AppHost::preload`]) is a task of that runtime, lease and all: it releases
//!   its admission when the load ends, not when the caller stops waiting.
//! - **Events.** What the call tells the page (its turns, its states, what was said, the devices when they change) is
//!   handed to [`EventSink`] as JSON, in the order the call made it, with the generation of the page it is for
//!   ([`PageGeneration`]). A page owns its lanes, its commands and its events: once the next page is the current one,
//!   the old one's commands are refused (`page_stale`), its lanes are released, and nothing they still say reaches the sink.
//! - **Lifecycle.** [`AppHost::page_changed`] when the page that owns the calls is gone (it answers the next page's number), and [`AppHost::close`] before the
//!   process exits, bounded, so every model and device is released before the C++ libraries' static destructors run.
//!
//! The commands of each concern are in `app/voice.rs` and `app/engine.rs`; they refuse as the bridge does
//! (docs/BRIDGE.md → "Refusals").

mod engine;
mod keychain;
#[cfg(test)]
mod tests;
mod voice;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sidevoice_voice::{AudioDevices, NativeDevices, SharedNativeAudio};
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use crate::engine::{Jobs, NativeEngines};
use crate::voice::host::{LaneAudio, Refusal, VoiceState};

pub use keychain::Keychain;
pub use voice::AudioChoice;

/// How often the devices are listed to tell the page when they change: the system tells cpal of no change.
const DEVICES_EVERY: Duration = Duration::from_secs(2);

/// How often `close` looks again for models still held.
const RELEASE_POLL: Duration = Duration::from_millis(50);

/// What the app tells the host about itself.
pub struct HostConfig {
    /// Where the engine keeps its models: shared by every app of this person on this machine (docs/ENGINES.md).
    pub model_root: PathBuf,
}

/// What the shell implements.
pub struct Hooks {
    pub admission: Arc<dyn Admission>,
    pub events: Arc<dyn EventSink>,
    pub keys: Arc<dyn ProviderKeys>,
}

/// Lets work use the app's data, or refuses it while the data is being changed.
pub trait Admission: Send + Sync + 'static {
    /// A lease to hold while the work lasts, or `None` while a change of the data is under way.
    fn admit(&self) -> Option<Lease>;
}

/// Held for as long as admitted work lasts, and let go when dropped: it can cross to a blocking thread and an await.
pub struct Lease {
    _held: Box<dyn Send + 'static>,
}

impl Lease {
    pub fn new(held: impl Send + 'static) -> Lease {
        Lease { _held: Box::new(held) }
    }
}

/// An app that never changes its data under its work: every request is admitted.
pub struct AlwaysAdmit;

impl Admission for AlwaysAdmit {
    fn admit(&self) -> Option<Lease> {
        Some(Lease::new(()))
    }
}

/// Which page, of those that came and went, something is for or from. The host counts them: the first page is 1 (0 is the
/// time before any), each [`AppHost::page_changed`] makes the next, and only the current one has lanes, events and
/// commands that are heard. A shell hands each page its number ([`AppHost::page`]) and takes it back on every command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PageGeneration(pub u64);

/// Where the call's events go: the page that owns the calls, as JSON (`{key, type, data?}`, docs/BRIDGE.md → "The voice
/// call"). `page` is the generation they were made for: the host sends none for a page that is no longer the current one,
/// and a sink that keeps a channel per page should check it too, since the current page can change as an event is on
/// its way.
pub trait EventSink: Send + Sync + 'static {
    fn event(&self, page: PageGeneration, event: Value);
}

/// Where provider keys are kept (docs/CREDENTIALS.md). Never the store's own error text: it can hold secret bytes.
pub trait ProviderKeys: Send + Sync + 'static {
    /// Whether `provider` is a well-formed id: a key of the store, not necessarily a member of today's catalogue.
    fn valid(&self, provider: &str) -> bool;
    /// A provider's key; `None` when it has none.
    fn read(&self, provider: &str) -> Result<Option<String>, KeyError>;
    /// Replaces a provider's key, or removes it for `None`.
    fn write(&self, provider: &str, key: Option<&str>) -> Result<(), KeyError>;
}

/// Why the store would not answer, in categories that carry no secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    Unavailable,
    InvalidProvider,
    TooLong,
    Failed,
}

/// The application: its calls, its engine, its devices.
pub struct AppHost<A: LaneAudio = SharedNativeAudio> {
    inner: Arc<Inner<A>>,
}

impl<A: LaneAudio> Clone for AppHost<A> {
    fn clone(&self) -> Self {
        AppHost { inner: Arc::clone(&self.inner) }
    }
}

struct Inner<A: LaneAudio> {
    runtime: Handle,
    hooks: Hooks,
    voice: VoiceState<A>,
    /// `None` when the engine did not start; its commands, and the call's models, refuse then.
    engines: Option<Arc<NativeEngines>>,
    jobs: Arc<Jobs>,
    /// The current page's generation (shared with the sinks of the calls, which drop what is for another).
    generation: Arc<AtomicU64>,
    /// The task that tells the page when the devices change, from the first time the page asks for them.
    watcher: Mutex<Option<JoinHandle<()>>>,
}

impl AppHost {
    /// The host on the device's own microphone and speaker. Not from inside the runtime's tasks: the engine starts by
    /// blocking on it.
    pub fn new(config: HostConfig, runtime: Handle, hooks: Hooks) -> AppHost {
        // The shared audio starts its dispatcher on Tokio.
        let voice = {
            let _inside = runtime.enter();
            VoiceState::default()
        };
        AppHost::assemble(config, runtime, hooks, voice)
    }
}

impl<A: LaneAudio> AppHost<A> {
    /// The host on another `audio`, as a test gives it, with the `devices` it lists.
    pub fn with_audio(config: HostConfig, runtime: Handle, hooks: Hooks, audio: A, devices: NativeDevices) -> Self {
        let voice = {
            let _inside = runtime.enter();
            VoiceState::new(audio, devices)
        };
        AppHost::assemble(config, runtime, hooks, voice)
    }

    fn assemble(config: HostConfig, runtime: Handle, hooks: Hooks, voice: VoiceState<A>) -> Self {
        let keys = engine::EngineKeys::new(Arc::clone(&hooks.keys), runtime.clone());
        let engines = match NativeEngines::new(config.model_root, keys, runtime.clone()) {
            Ok(engines) => Some(Arc::new(engines)),
            Err(e) => {
                log::error!("the native engine did not start: {} ({})", e.key, e.message);
                None
            }
        };
        let inner = Inner {
            runtime,
            hooks,
            voice,
            engines,
            jobs: Arc::default(),
            generation: Arc::default(),
            watcher: Mutex::default(),
        };
        AppHost { inner: Arc::new(inner) }
    }

    /// The current page's generation: what a page is given to send back with its commands.
    pub fn page(&self) -> PageGeneration {
        PageGeneration(self.inner.generation.load(Ordering::SeqCst))
    }

    /// The page that owned the calls is gone (it reloaded): a new page owns none of its previous lanes, and is the current
    /// one from now on. The old lanes (only those: a lane the new page has made already stays) are stopped and released in
    /// the background, and keep their models for their idle minutes; nothing they still say reaches the sink, and commands
    /// of the old page are refused (`page_stale`).
    pub fn page_changed(&self) -> PageGeneration {
        let next = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let inner = Arc::clone(&self.inner);
        self.inner.runtime.spawn(async move { inner.voice.clear_older_than(next).await });
        PageGeneration(next)
    }

    /// Quitting or restarting, before `exit()` runs the static destructors of the C++ libraries the models are made of:
    /// every call is stopped and released, so the devices close, and every model is let go, those a stopped call or a
    /// preload kept for their idle minutes too. One left loaded is still alive in those destructors, and whisper.cpp's
    /// Metal device aborts in its own when one is (#121). Bounded: it returns the models still held when `limit` passed
    /// (by their key), logged, and the quit goes on; none once every one was released.
    pub async fn close(&self, limit: Duration) -> Vec<String> {
        if let Some(watcher) = self.inner.watcher.lock().unwrap().take() {
            watcher.abort();
        }
        let held = self.entered(self.inner.voice.close(limit, RELEASE_POLL)).await;
        if held.is_empty() {
            crate::debug::debug("voice: every model released");
        } else {
            log::error!("voice: models still held at exit after {limit:?}: {}", held.join(", "));
            crate::debug::debug(&format!("voice: models still held at exit: {}", held.join(", ")));
        }
        held
    }

    /// CI's memory run (src/probe.rs): what the call host holds now, as `keeps=N lanes=M`.
    #[cfg(feature = "probe")]
    pub fn held(&self) -> String {
        self.inner.voice.held()
    }

    /// `work` polled with the host's runtime entered: whatever it spawns, times or asks the current runtime for is the
    /// host's, whichever executor the caller polls it on. (The runtime handle the shell passed in is the only one the host
    /// uses; the ambient one, if any, is not.)
    fn entered<F: Future>(&self, work: F) -> Entered<F> {
        Entered { runtime: self.inner.runtime.clone(), work: Box::pin(work) }
    }

    /// `work` run as a task of the host's runtime, and its answer: the work goes on, lease and all, if the caller gives up
    /// waiting for it. For work that loads models on blocking threads, which an admission must outlast.
    async fn detached<T: Send + 'static>(
        &self,
        work: impl Future<Output = Result<T, Refusal>> + Send + 'static,
    ) -> Result<T, Refusal> {
        self.inner.runtime.spawn(work).await.map_err(|e| crate::voice::host::refusal("internal", e.to_string()))?
    }

    /// The events sink as the `Page` of a call of generation `page`: what it says reaches the sink while that is the current
    /// page, and never after.
    fn page_sink(&self, page: PageGeneration) -> crate::voice::host::Page {
        let (events, current) = (Arc::clone(&self.inner.hooks.events), Arc::clone(&self.inner.generation));
        Arc::new(move |event| {
            if current.load(Ordering::SeqCst) == page.0 {
                events.event(page, event);
            }
        })
    }

    /// Refuses a command of a page that is not the current one.
    fn current(&self, page: PageGeneration) -> Result<(), Refusal> {
        if page == self.page() {
            Ok(())
        } else {
            Err(crate::voice::host::refusal("page_stale", "That page has been replaced by a newer one."))
        }
    }

    /// Lists the devices every `DEVICES_EVERY` from the first call on, and tells the page each change: other devices,
    /// another choice, or a chosen one that went away.
    fn watch_devices(&self) {
        let state = &self.inner.voice;
        if state.watching.swap(true, Ordering::AcqRel) {
            return;
        }
        let (events, devices) = (Arc::clone(&self.inner.hooks.events), state.devices.clone());
        let current = Arc::clone(&self.inner.generation);
        let watcher = self.inner.runtime.spawn(async move {
            let mut last: Option<AudioDevices> = None;
            loop {
                let listing = devices.clone();
                let Ok(Ok(now)) = tokio::task::spawn_blocking(move || listing.list()).await else {
                    tokio::time::sleep(DEVICES_EVERY).await;
                    continue;
                };
                let changed = now.lost.is_some()
                    || last.as_ref().is_some_and(|last| {
                        (&last.inputs, &last.outputs, &last.input, &last.output)
                            != (&now.inputs, &now.outputs, &now.input, &now.output)
                    });
                if changed {
                    events.event(
                        PageGeneration(current.load(Ordering::SeqCst)),
                        json!({"type": "devices", "data": voice::seam_devices(&now)}),
                    );
                }
                last = Some(now);
                tokio::time::sleep(DEVICES_EVERY).await;
            }
        });
        *self.inner.watcher.lock().unwrap() = Some(watcher);
    }
}

/// A future polled with a runtime entered (`AppHost::entered`).
struct Entered<F: Future> {
    runtime: Handle,
    work: Pin<Box<F>>,
}

impl<F: Future> Future for Entered<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let this = &mut *self;
        let _inside = this.runtime.enter();
        this.work.as_mut().poll(cx)
    }
}
