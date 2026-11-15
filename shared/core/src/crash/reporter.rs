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
use crate::logging::Resource;
use sentry::types::Dsn;
use sentry::{Client, ClientOptions};
use std::path::PathBuf;
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

pub struct Reporter {
    resource: Resource,
    dsn: Option<Dsn>,
    post: Option<Arc<dyn Post>>,
    settings: Mutex<Settings>,
    live: Mutex<Option<Live>>,
    /// One machine record at a time: its outcome is read from the gate.
    sending: Mutex<()>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

impl Reporter {
    /// Reads the setting and, under `auto` with somewhere to send to, makes the client.
    pub fn start(config: Config) -> Arc<Reporter> {
        let dsn = config.dsn.as_deref().filter(|text| !text.is_empty()).and_then(|text| text.parse::<Dsn>().ok());
        let settings = Settings::load(&config.dir);
        let reporter = Arc::new(Reporter {
            resource: config.resource,
            dsn,
            post: config.post,
            settings: Mutex::new(settings),
            live: Mutex::new(None),
            sending: Mutex::new(()),
        });
        if reporter.mode() == Mode::Auto {
            reporter.go_live();
        }
        reporter
    }

    pub fn mode(&self) -> Mode {
        self.settings.lock().unwrap_or_else(PoisonError::into_inner).mode()
    }

    /// Whether anything can be sent now: the mode is `auto` and there is somewhere to send to.
    pub fn is_active(&self) -> bool {
        self.live.lock().unwrap_or_else(PoisonError::into_inner).is_some()
    }

    fn go_live(&self) {
        let post: Arc<dyn Post> = match (&self.post, &self.dsn) {
            (Some(post), _) => post.clone(),
            (None, Some(dsn)) => {
                Arc::new(SentryPost::new(dsn, &format!("sidevoice/{}", self.resource.service_version)))
            }
            (None, None) => return,
        };
        let gate = Arc::new(Gate::new(post));
        let dsn = self.dsn.clone().or_else(|| TEST_DSN.parse().ok());
        // `ClientOptions::default()` reads SENTRY_* variables: every field that matters is set here, and `sanitize` is the
        // last word on the event whatever the options say.
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
        *self.live.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(Live { client: Arc::new(Client::from(options)), gate });
    }

    fn stop(&self) {
        let live = self.live.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(live) = live {
            live.gate.close();
            live.client.close(Some(Duration::ZERO));
        }
    }

    /// The person's choice. `never` takes effect first and whatever happens after (the file may not be writable); `auto`
    /// only once it is kept. An error means the choice was not kept.
    pub fn set_mode(&self, mode: Mode) -> std::io::Result<()> {
        if mode == Mode::Never {
            self.stop();
        }
        let kept = self.settings.lock().unwrap_or_else(PoisonError::into_inner).set_mode(mode);
        if mode == Mode::Auto && kept.is_ok() && !self.is_active() {
            self.go_live();
        }
        kept
    }

    /// The app's own panic, from the panic hook: never panics, never waits for a lock the panicking thread may hold, and
    /// does nothing under `never`. `literal` is the payload only when it is a `&'static str`.
    pub fn capture_panic(&self, literal: Option<&str>, thread: &str) {
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
        let event = event::own_panic(
            &self.resource,
            &installation,
            OwnPanic {
                literal,
                thread,
                stacktrace: sentry::integrations::backtrace::current_stacktrace(),
                images: sentry::integrations::debug_images::debug_images(),
            },
        );
        client.capture_event(event, None);
    }

    /// A machine's record as an event of its own. `Ok` only when it was delivered.
    pub fn send_machine(&self, record: &MachineRecord) -> Result<(), SendError> {
        let _one = self.sending.lock().unwrap_or_else(PoisonError::into_inner);
        let (client, gate) = {
            let live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
            let live = live.as_ref().ok_or(SendError::Disabled)?;
            (live.client.clone(), live.gate.clone())
        };
        gate.take_outcome();
        client.capture_event(event::machine_record(record), None);
        match gate.take_outcome() {
            Some(true) => Ok(()),
            Some(false) => Err(SendError::Failed),
            None => Err(SendError::Dropped),
        }
    }
}
