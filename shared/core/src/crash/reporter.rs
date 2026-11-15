//! The reporter: the setting, the client while the setting is `auto`, and the two ways in (the app's own panic, a machine's
//! record).
//!
//! Under `never` there is no client: nothing is initialised, nothing is sent, and switching to it closes the transport's gate
//! first (so nothing that is being handed over leaves), drops the client with whatever it held, and forgets the installation
//! id. There is no queue to drain and no cache on disk. With no DSN in the build (a local or dev build) the setting still
//! works and nothing is ever sent.

use super::event::{self, OwnPanic};
use super::settings::Settings;
use super::transport::{Gate, Post, SentryPost};
use super::{machine::MachineRecord, Mode};
use crate::error::SharedError;
use crate::logging::Resource;
use sentry::protocol::{DebugImage, Stacktrace};
use sentry::types::Dsn;
use sentry::{Client, ClientOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A DSN for a test transport that has no real one.
const TEST_DSN: &str = "https://public@sentry.invalid/1";

pub struct Config {
    /// What the app is: its name, version, OS and architecture, for its own events.
    pub resource: Resource,
    /// The DSN baked into a release build; `None` (or empty) in a local or dev build, which never sends.
    pub dsn: Option<String>,
    /// Where the setting is kept (the app's own directory).
    pub dir: PathBuf,
    /// A transport of the caller's instead of the POST to Sentry, for tests.
    pub post: Option<Arc<dyn Post>>,
}

/// Why an event was not sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendError {
    /// The mode is `never`, or the build has no DSN.
    Disabled,
    /// The POST failed.
    Failed,
    /// The event was dropped before it was posted.
    Dropped,
}

struct Live {
    client: Arc<Client>,
    gate: Arc<Gate>,
}

/// The most own panics one run reports: a panic in a loop must not turn into a stream of sends.
const MAX_PANIC_REPORTS: usize = 10;

thread_local! {
    /// Set while this thread is inside the reporter's capture of a panic: a panic raised there (or a transport that reports
    /// again) is not reported again.
    static CAPTURING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub struct Reporter {
    resource: Resource,
    dsn: Option<Dsn>,
    post: Option<Arc<dyn Post>>,
    settings: Mutex<Settings>,
    live: Mutex<Option<Live>>,
    panics_reported: AtomicUsize,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

impl Reporter {
    fn with(resource: Resource, dsn: Option<Dsn>, post: Option<Arc<dyn Post>>, settings: Settings) -> Arc<Reporter> {
        Arc::new(Reporter {
            resource,
            dsn,
            post,
            settings: Mutex::new(settings),
            live: Mutex::new(None),
            panics_reported: AtomicUsize::new(0),
        })
    }

    /// A reporter that reports nothing and keeps nothing (`never` for this run), for when the app's directory could not be had
    /// or the reporter could not start: the app and its page work the same.
    pub fn unavailable(resource: Resource) -> Arc<Reporter> {
        Reporter::with(resource, None, None, Settings::unavailable())
    }

    /// Reads the setting and, under `auto` with somewhere to send to, makes the client. Never fails and never panics: when
    /// anything goes wrong the reporter is [`Reporter::unavailable`] for this run.
    pub fn start(config: Config) -> Arc<Reporter> {
        Reporter::begin(config, false)
    }

    /// As [`Reporter::start`], with the client made on a thread of its own: only the small settings file is read on the
    /// caller's thread, so the app's start does not wait for the rest.
    pub fn start_in_background(config: Config) -> Arc<Reporter> {
        Reporter::begin(config, true)
    }

    fn begin(config: Config, background: bool) -> Arc<Reporter> {
        let resource = config.resource;
        let made = super::contained("start", || {
            let dsn = config.dsn.as_deref().filter(|text| !text.is_empty()).and_then(|text| text.parse::<Dsn>().ok());
            let reporter = Reporter::with(config.resource, dsn, config.post, Settings::load(&config.dir));
            if reporter.mode() == Mode::Auto {
                if background {
                    let live = reporter.clone();
                    let spawned = std::thread::Builder::new().name("crash-init".into()).spawn(move || live.go_live());
                    if spawned.is_err() {
                        tracing::warn!(
                            error.type = SharedError::CrashReporterFailed.code(),
                            "the crash reporter could not start its thread; nothing is reported in this run"
                        );
                    }
                } else {
                    reporter.go_live();
                }
            }
            reporter
        });
        made.unwrap_or_else(|| Reporter::unavailable(resource))
    }

    pub fn mode(&self) -> Mode {
        self.settings.lock().unwrap_or_else(PoisonError::into_inner).mode()
    }

    /// Whether anything can be sent now: the mode is `auto` and there is somewhere to send to.
    pub fn is_active(&self) -> bool {
        self.live.lock().unwrap_or_else(PoisonError::into_inner).is_some()
    }

    /// [`Reporter::is_active`] for the panic hook: never waits for a lock the panicking thread may hold.
    pub(super) fn would_send(&self) -> bool {
        self.live.try_lock().is_ok_and(|live| live.as_ref().is_some_and(|live| live.gate.is_open()))
    }

    fn go_live(&self) {
        super::contained("go-live", || self.make_client());
    }

    fn make_client(&self) {
        let post: Arc<dyn Post> = match (&self.post, &self.dsn) {
            (Some(post), _) => post.clone(),
            (None, Some(dsn)) => {
                Arc::new(SentryPost::new(dsn, &format!("sidevoice/{}", self.resource.service_version)))
            }
            (None, None) => return,
        };
        let gate = Arc::new(Gate::new(post));
        let dsn = self.dsn.clone().or_else(|| TEST_DSN.parse().ok());
        // `ClientOptions::default()` reads no environment variable (the SENTRY_* ones are `sentry::apply_defaults`'s, which is
        // not called here); every field that matters is set here anyway, and `sanitize` is the last word on the event whatever the options say.
        let mut options = ClientOptions::default();
        options.dsn = dsn;
        options.release = None;
        options.environment = None;
        options.server_name = None;
        options.send_default_pii = false;
        options.default_integrations = false;
        options.integrations = Vec::new();
        options.max_breadcrumbs = 0;
        options.attach_stacktrace = false;
        options.auto_session_tracking = false;
        options.debug = false;
        options.transport = Some(Arc::new(gate.clone()));
        options.before_send = Some(Arc::new(|event| Some(event::sanitize(event))));
        options.before_breadcrumb = Some(Arc::new(|_| None));
        let client = Arc::new(Client::from(options));
        // The mode is looked at again with the slot held: the person may have chosen `never` while this client was made
        // (this runs on a thread of its own at start), and `never` is never overtaken.
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        if live.is_some() || self.mode() != Mode::Auto {
            gate.close();
            return;
        }
        *live = Some(Live { client, gate });
    }

    /// The installation id the app's own reports carry, made if there is none yet (what the page shows is what is sent).
    /// `None` under `never`: the id is forgotten then.
    pub fn installation_id(&self) -> Option<String> {
        super::contained("installation-id", || {
            let mut settings = self.settings.lock().unwrap_or_else(PoisonError::into_inner);
            if settings.mode() == Mode::Never {
                return None;
            }
            settings.install_id(now()).ok()
        })
        .flatten()
    }

    /// The gate of the client now held, for the tests that look at what a switch does to it.
    #[cfg(test)]
    pub(super) fn gate(&self) -> Option<Arc<Gate>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|live| live.gate.clone())
    }

    fn stop(&self) {
        super::contained("stop", || {
            let live = self.live.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(live) = live {
                live.gate.close();
                live.client.close(Some(Duration::ZERO));
            }
        });
    }

    /// The person's choice. `never` takes effect first and whatever happens after (the file may not be writable); `auto`
    /// only once it is kept. An error means the choice was not kept.
    pub fn set_mode(&self, mode: Mode) -> std::io::Result<()> {
        super::contained("set-mode", || {
            if mode == Mode::Never {
                self.stop();
            }
            let kept = self.settings.lock().unwrap_or_else(PoisonError::into_inner).set_mode(mode);
            if mode == Mode::Never {
                // A client that was still being made at the first `stop` is gone now too.
                self.stop();
            } else if kept.is_ok() && !self.is_active() {
                self.go_live();
            }
            kept
        })
        .unwrap_or_else(|| Err(std::io::Error::other("the crash reporter failed")))
    }

    /// The app's own panic: never panics, never waits for a lock the panicking thread may hold, and does nothing under
    /// `never`. `literal` is the payload only when it is a `&'static str`. The frames are taken on the calling thread; the
    /// panic hook takes them itself and calls [`Reporter::capture_panic_with`].
    pub fn capture_panic(&self, literal: Option<&str>, thread: &str) {
        if !self.would_send() {
            return;
        }
        let stacktrace = super::contained("frames", sentry::integrations::backtrace::current_stacktrace).flatten();
        let images = super::contained("modules", sentry::integrations::debug_images::debug_images).unwrap_or_default();
        self.capture_panic_with(literal, thread, stacktrace, images);
    }

    /// [`Reporter::capture_panic`] with the frames and modules already taken. At most [`MAX_PANIC_REPORTS`] a run, and a call
    /// from inside a capture (a transport that reports again) returns at once.
    pub fn capture_panic_with(
        &self,
        literal: Option<&str>,
        thread: &str,
        stacktrace: Option<Stacktrace>,
        images: Vec<DebugImage>,
    ) {
        if CAPTURING.with(|flag| flag.replace(true)) {
            return;
        }
        super::contained("capture-panic", || {
            if self.panics_reported.load(Ordering::SeqCst) >= MAX_PANIC_REPORTS {
                return;
            }
            let (client, gate) = {
                let Ok(live) = self.live.try_lock() else { return };
                let Some(live) = live.as_ref() else { return };
                (live.client.clone(), live.gate.clone())
            };
            if !gate.is_open() {
                return;
            }
            let Ok(mut settings) = self.settings.try_lock() else { return };
            let Ok(installation) = settings.install_id(now()) else { return };
            drop(settings);
            self.panics_reported.fetch_add(1, Ordering::SeqCst);
            let event =
                event::own_panic(&self.resource, &installation, OwnPanic { literal, thread, stacktrace, images });
            client.capture_event(event, None);
        });
        CAPTURING.with(|flag| flag.set(false));
    }

    /// A machine's record as an event of its own. `Ok` only when it was delivered.
    pub fn send_machine(&self, record: &MachineRecord) -> Result<(), SendError> {
        super::contained("send-machine", || {
            let (client, gate) = {
                let live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
                let live = live.as_ref().ok_or(SendError::Disabled)?;
                (live.client.clone(), live.gate.clone())
            };
            // The outcome is the event's own (by its id): an app panic posted meanwhile cannot answer for it.
            gate.watch(record.id);
            client.capture_event(event::machine_record(record), None);
            match gate.take_outcome(record.id) {
                Some(true) => Ok(()),
                Some(false) => Err(SendError::Failed),
                None => Err(SendError::Dropped),
            }
        })
        .unwrap_or(Err(SendError::Failed))
    }
}
