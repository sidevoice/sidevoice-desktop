//! Crash reports (Sentry), shared by the desktop and the phone: decided with the operator (2026-11-15, 2026-11-17; tracked in
//! sidevoice-desktop#523) and written down in the logging guideline, sections 9 and 10.
//!
//! What the app does when it panics is one setting the app owns, [`Mode`]: send automatically (the default, never asked) or
//! never. Each piece sends **its own** crashes, and only its own: the app does not read, relay or acknowledge anything of another
//! piece's, and no machine does so for the app.
//!
//! - **At panic time** the log's panic hook tells [`Reporter::record_panic`], which writes a record ([`record`]) in the app's own
//!   directory: the literal message or the marker, the code location, the frames with their raw addresses, the loaded modules
//!   that hold a frame, and the last warnings and errors of our own targets from an in-memory ring ([`tail`]). Nothing is sent
//!   from the hook. Under "never" no record is made.
//! - **At the next start** a thread of its own sends each record ([`Reporter::start`]) as an event built from the record's
//!   closed field list ([`report`]) and deletes it once Sentry took it, so it is sent once. Under "never" the pending records
//!   are discarded instead.
//! - **Handled errors** ([`report_handled_errors`], `handled.rs`) go out under the same switch: the `error` events of our own
//!   targets whose `error.type` is one of the app's own codes, one event per code per day, each with its code and the place that
//!   logged it and the closed tags, never a message or a field value.
//!
//! What leaves is the closed list of `report.rs` (and, for a handled error, of `handled.rs`); nothing that is not on it is sent.
//! Formatted panic messages are never sent, only a literal or the fixed marker.
//!
//! - [`settings`]: the mode and the random installation id (pseudonymous, rotated yearly), in one small local file.
//! - [`record`]: the record and the checks it passes before it is kept or sent.
//! - [`store`]: the records on disk, bounded.
//! - [`tail`]: the last 20 warnings and errors of our targets, in memory.
//! - [`trace`]: the frames and the loaded modules.
//! - [`report`]: the event, the envelope and the POST.
//! - [`handled`]: the handled errors, sent under the same switch.

mod core_copy;
mod handled;
pub mod record;
pub mod report;
pub mod settings;
mod store;
pub mod tail;
#[cfg(test)]
mod tests;
mod trace;

use std::cell::Cell;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use core_copy::CoreCopy;
pub use handled::{layer as handled_layer, unreported as unreported_errors, MAX_CODES};
pub use report::{Post, PostError, SentryPost};
pub use settings::Settings;

use crate::error::SharedError;
use crate::logging::{Panicked, Resource};
use record::{CrashRecord, Image, Panic};
use store::Store;

/// The last warnings and errors a record carries.
pub const TAIL_LINES: usize = 20;
/// The most frames of the backtrace a record carries.
pub const MAX_FRAMES: usize = 64;
/// The most loaded modules a record carries: the ones that hold a frame.
pub const MAX_IMAGES: usize = 16;
/// The most records one run makes: a panic in a loop must not fill the disk or turn into a stream of reports.
const MAX_PER_RUN: usize = 5;
/// How long the panic hook waits for the capture thread in all. A panic inside a hook aborts the process and cannot be caught,
/// so the capture runs on a thread of its own and the hook only waits, at most this long.
const HOOK_WAIT: Duration = Duration::from_secs(3);
/// How long one POST may take, and a whole pass: what is left waits for the next start.
const POST_BUDGET: Duration = Duration::from_secs(10);
const PASS_BUDGET: Duration = Duration::from_secs(60);

/// The log targets whose lines are never taken from the log for a report, a crash record's tail or a handled error: the panic's two
/// (the record says what the panic was, and a handled error is never one of those) and the page's (what a page says is not ours to
/// send). Written out, not left to omission: filters match targets by prefix.
pub(crate) const NOT_REPORTED: [&str; 3] =
    [crate::logging::PANIC_TARGET, crate::logging::BACKTRACE_TARGET, crate::logging::WEBVIEW_TARGET];

/// What happens when the app panics (Settings → Crash reports).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Send crash reports automatically. The default.
    #[default]
    Auto,
    /// Send nothing.
    Never,
}

impl Mode {
    /// The name the page uses (`auto`, `never`).
    pub fn name(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Never => "never",
        }
    }

    pub fn parse(name: &str) -> Option<Mode> {
        match name {
            "auto" => Some(Mode::Auto),
            "never" => Some(Mode::Never),
            _ => None,
        }
    }
}

/// Runs `step` so that a panic in it ends there: the reporter is best-effort and never takes the app down. A panic is logged (a
/// fixed sentence and `error.type`; the step is a fixed name, the payload is never read) and `None` comes back.
pub(crate) fn contained<T>(step: &'static str, run: impl FnOnce() -> T) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(value) => Some(value),
        Err(_) => {
            tracing::warn!(
                error.type = SharedError::CrashReporterFailed.code(),
                step,
                "a step of the crash reporter failed and was contained; the app goes on"
            );
            None
        }
    }
}

thread_local! {
    /// Set on the thread that sends: a panic of the reporter itself is logged and contained, never recorded (no loop).
    static SENDING: Cell<bool> = const { Cell::new(false) };
}

/// Spawns a thread whose panics are the reporter's own: they are logged where they are caught, never recorded as a crash of the
/// app (no loop). Every thread the reporter starts, and the ones its transport runs on, is one.
pub fn reporter_thread<T: Send + 'static>(
    name: &str,
    run: impl FnOnce() -> T + Send + 'static,
) -> io::Result<std::thread::JoinHandle<T>> {
    std::thread::Builder::new().name(name.into()).spawn(move || {
        mark_reporter_thread();
        run()
    })
}

/// The calling thread is the reporter's: see [`reporter_thread`]. For the threads a library starts for the reporter's work (a
/// runtime's workers).
pub(crate) fn mark_reporter_thread() {
    SENDING.with(|flag| flag.set(true));
}

/// One capture at a time, process-wide: a panic that comes while one is under way (another thread's, or the capture's own) is
/// not recorded.
static CAPTURING: AtomicBool = AtomicBool::new(false);

struct Release;

impl Drop for Release {
    fn drop(&mut self) {
        CAPTURING.store(false, Ordering::SeqCst);
    }
}

pub struct Config {
    /// What the app is: its name, version, OS and architecture, for its own records.
    pub resource: Resource,
    /// The DSN baked into a release build; `None` (or empty) in a local or dev build, which records and sends nothing.
    pub dsn: Option<String>,
    /// Where the setting and the records are kept (the app's own directory).
    pub dir: PathBuf,
    /// A transport of the caller's instead of the POST to Sentry, for tests.
    pub post: Option<Arc<dyn Post>>,
}

/// Told when a copy of the switch becomes owed to the core the app installed.
type Copier = Arc<dyn Fn() + Send + Sync>;

/// What the core is owed: the person's choice as it is now, and which owing it answers (a copy that comes back after the person
/// changed the switch again does not settle the new one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owed {
    pub mode: Mode,
    generation: u64,
}

pub struct Reporter {
    resource: Resource,
    /// The way out: the POST to the DSN, or a test's. `None` in a build with no DSN.
    post: Option<Arc<dyn Post>>,
    /// Where records are kept. `None` when the app's directory could not be had.
    store: Option<Store>,
    settings: Mutex<Settings>,
    /// Whether anything may be recorded or sent now: the mode is `auto` and the build can send. Closed first when the person
    /// chooses `never`, whatever happens after (the file may not be writable).
    open: AtomicBool,
    /// The modules loaded at start, of which a record keeps the ones that hold a frame.
    images: Vec<Image>,
    /// Whether a pass over the records is under way.
    sending: AtomicBool,
    /// How many records this run made.
    recorded: AtomicUsize,
    /// Whether the app owes the core it installed a copy of its switch (`core_copy.rs`), and how many times it came to owe one
    /// in this run. Locked after `settings` where both are.
    copy: Mutex<CoreCopy>,
    owings: AtomicU64,
    copier: Mutex<Option<Copier>>,
    /// The handled error types sent today (`handled.rs`), in memory and in a file beside the setting. Locked on its own: nothing else
    /// is held while it is.
    handled: Mutex<handled::Cap>,
}

impl Reporter {
    fn with(
        resource: Resource,
        post: Option<Arc<dyn Post>>,
        store: Option<Store>,
        settings: Settings,
        copy: CoreCopy,
        images: Vec<Image>,
        dir: PathBuf,
    ) -> Arc<Reporter> {
        let open = settings.mode() == Mode::Auto && post.is_some() && store.is_some();
        Arc::new(Reporter {
            resource,
            post,
            store,
            settings: Mutex::new(settings),
            open: AtomicBool::new(open),
            images,
            sending: AtomicBool::new(false),
            recorded: AtomicUsize::new(0),
            copy: Mutex::new(copy),
            owings: AtomicU64::new(0),
            copier: Mutex::new(None),
            handled: Mutex::new(handled::Cap::at(dir)),
        })
    }

    /// A reporter that reports nothing and keeps nothing (`never` for this run), for when the app's directory could not be had or
    /// the reporter could not start: the app and its page work the same.
    pub fn unavailable(resource: Resource) -> Arc<Reporter> {
        Reporter::with(
            resource,
            None,
            None,
            Settings::unavailable(),
            CoreCopy::unavailable(),
            Vec::new(),
            PathBuf::new(),
        )
    }

    /// Reads the setting and lists the loaded modules, then, on a thread of its own, sends the records a previous run left (or
    /// discards them under `never`). Never fails, never blocks and never panics: when anything goes wrong the reporter is
    /// [`Reporter::unavailable`] for this run.
    pub fn start(config: Config) -> Arc<Reporter> {
        let resource = config.resource;
        let made = contained("start", || {
            let post = config.post.or_else(|| {
                let dsn = config.dsn.as_deref().filter(|text| !text.is_empty()).and_then(report::parse_dsn)?;
                let post: Arc<dyn Post> =
                    Arc::new(SentryPost::new(&dsn, &format!("sidevoice/{}", resource.service_version)));
                Some(post)
            });
            let store = Store::new(&config.dir, crate::logging::home());
            let settings = Settings::load(&config.dir);
            let reporter = Reporter::with(
                resource,
                post,
                Some(store),
                settings,
                CoreCopy::load(&config.dir),
                trace::loaded(),
                config.dir.clone(),
            );
            reporter.begin_pass();
            reporter
        });
        made.unwrap_or_else(|| Reporter::unavailable(resource))
    }

    pub fn mode(&self) -> Mode {
        self.settings.lock().unwrap_or_else(PoisonError::into_inner).mode()
    }

    /// Whether anything may be recorded or sent now.
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    /// The installation id the app's reports carry, made if there is none yet (what the page shows is what is sent). `None`
    /// under `never`: the id is forgotten then.
    pub fn installation_id(&self) -> Option<String> {
        contained("installation-id", || {
            let mut settings = self.settings.lock().unwrap_or_else(PoisonError::into_inner);
            if settings.mode() == Mode::Never {
                return None;
            }
            settings.install_id(now_secs()).ok()
        })
        .flatten()
    }

    /// The person's choice. `never` takes effect first and whatever happens after (the file may not be writable): nothing more
    /// is recorded or sent, and the pending records and the installation id are deleted; `auto` only once it is kept. An error
    /// means the choice was not kept. A change, once the app installed a core, makes a copy of the switch owed to it
    /// ([`Reporter::on_copy_owed`]).
    pub fn set_mode(&self, mode: Mode) -> io::Result<()> {
        contained("set-mode", || {
            if mode == Mode::Never {
                self.open.store(false, Ordering::SeqCst);
            }
            // The mode it was, the write and the owing under one hold: two changes at once cannot both see the same "before".
            let (kept, changed, resolved) = {
                let mut settings = self.settings.lock().unwrap_or_else(PoisonError::into_inner);
                let (previous, unknown) = (settings.mode(), settings.unknown());
                let kept = settings.set_mode(mode);
                let changed = kept.is_ok() && previous != mode;
                if changed {
                    self.owe_copy(|copy| copy.owe());
                }
                // A choice saved over a setting that could not be read: what was owed and held back can go now.
                let resolved = kept.is_ok() && unknown;
                (kept, changed, resolved)
            };
            match mode {
                Mode::Never => self.erase(),
                Mode::Auto if kept.is_ok() => {
                    self.open.store(self.post.is_some() && self.store.is_some(), Ordering::SeqCst);
                }
                Mode::Auto => {}
            }
            if changed || resolved {
                self.tell_copier();
            }
            kept
        })
        .unwrap_or_else(|| Err(io::Error::other("the crash reporter failed")))
    }

    /// The pending records and the day's list of handled errors deleted. A failure is reported and the switch stays off: the next
    /// start erases again.
    fn erase(&self) {
        if let Some(Err(error)) = self.store.as_ref().map(Store::erase) {
            tracing::warn!(
                error.type = SharedError::CrashRecordsUnavailable.code(),
                kind = ?error.kind(),
                "the crash records of an app set not to send could not be deleted"
            );
        }
        if let Err(error) = self.handled.lock().unwrap_or_else(PoisonError::into_inner).erase() {
            tracing::warn!(
                error.type = SharedError::CrashRecordsUnavailable.code(),
                kind = ?error.kind(),
                "the day's list of reported errors of an app set not to send could not be deleted"
            );
        }
    }

    /// What copies the app's switch to the core the app installed on this computer, told each time a copy becomes owed: when
    /// the app installed the core ([`Reporter::local_core_installed`]) and when the person changes the switch afterwards. It
    /// runs inside that call and must not block (start a thread); it asks [`Reporter::copy_owed`] what to send, and tells
    /// [`Reporter::copy_delivered`] when the core answered.
    pub fn on_copy_owed(&self, copy: impl Fn() + Send + Sync + 'static) {
        *self.copier.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(copy));
    }

    /// The app installed a core on this computer (a person's install, the first of a computer that had none): from now on a
    /// copy of the switch is owed to it, until the core answers one with a 200, and the person's changes of the switch owe it
    /// again. The copier is told.
    pub fn local_core_installed(&self) {
        contained("local-core", || {
            self.owe_copy(|copy| copy.installed());
            self.tell_copier();
        });
    }

    /// A copy of the switch is owed to the core the app installed: the person's choice as it is now (what is sent is always the
    /// reporter's mode at the time of sending: the latest wins), or `None` when nothing is owed, the app did not install the
    /// core, what is known of it could not be read, or **the setting could not be read**: then the mode is `never` only as a
    /// precaution, not the person's choice, so nothing is sent and the owed bit stays until a choice is saved.
    pub fn copy_owed(&self) -> Option<Owed> {
        contained("copy-owed", || {
            let settings = self.settings.lock().unwrap_or_else(PoisonError::into_inner);
            let copy = self.copy.lock().unwrap_or_else(PoisonError::into_inner);
            (copy.owed() && !settings.unknown())
                .then(|| Owed { mode: settings.mode(), generation: self.owings.load(Ordering::SeqCst) })
        })
        .flatten()
    }

    /// The core answered the copy of `owed` with a 200: nothing is owed any more, unless the person changed the switch since
    /// (that owes it again, and is not settled by this).
    pub fn copy_delivered(&self, owed: Owed) {
        contained("copy-delivered", || {
            let mut copy = self.copy.lock().unwrap_or_else(PoisonError::into_inner);
            if owed.generation == self.owings.load(Ordering::SeqCst) {
                if let Err(error) = copy.delivered() {
                    tracing::warn!(
                        error.type = SharedError::CrashSettingsNotKept.code(),
                        kind = ?error.kind(),
                        "the app could not keep that the core has its crash report switch; it will be sent once more when the core is next seen"
                    );
                }
            }
        });
    }

    /// Makes a copy owed (`how`), counted, and says so in the log when it could not be kept (it is owed in this run all the same).
    fn owe_copy(&self, how: impl FnOnce(&mut CoreCopy) -> io::Result<()>) {
        let mut copy = self.copy.lock().unwrap_or_else(PoisonError::into_inner);
        self.owings.fetch_add(1, Ordering::SeqCst);
        if let Err(error) = how(&mut copy) {
            tracing::warn!(
                error.type = SharedError::CrashSettingsNotKept.code(),
                kind = ?error.kind(),
                "the app could not keep that the core is owed a copy of its crash report switch; it is owed in this run only"
            );
        }
    }

    /// Tells the copier, if a copy is owed.
    fn tell_copier(&self) {
        let owed = self.copy.lock().unwrap_or_else(PoisonError::into_inner).owed();
        let copier = self.copier.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let (true, Some(copier)) = (owed, copier) {
            copier();
        }
    }

    /// Whether no pass over the records is under way, waiting at most `limit` for it to end: for tests and for an exit that
    /// would rather not cut a send short.
    pub fn wait_idle(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while self.sending.load(Ordering::SeqCst) {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }

    /// The pass over the records on a thread of its own, unless one is under way.
    fn begin_pass(self: &Arc<Self>) {
        if self.sending.swap(true, Ordering::SeqCst) {
            return;
        }
        let reporter = self.clone();
        let spawned = reporter_thread("crash-send", move || {
            contained("send", || reporter.pass());
            reporter.sending.store(false, Ordering::SeqCst);
        });
        if spawned.is_err() {
            self.sending.store(false, Ordering::SeqCst);
            tracing::warn!(
                error.type = SharedError::CrashReporterFailed.code(),
                "the crash reporter could not start its thread; nothing is sent in this run"
            );
        }
    }

    /// What a previous run left: sent one at a time, newest first, each deleted once Sentry took it; the first failure ends the
    /// pass (the network is likely down, and every record would wait out its timeout) and the rest wait for the next start.
    /// Under `never` the records are discarded instead.
    fn pass(&self) {
        let Some(store) = &self.store else { return };
        if self.mode() == Mode::Never {
            self.erase();
            return;
        }
        let Some(post) = &self.post else { return };
        let records = match store.pending() {
            Ok(records) => records,
            Err(error) => {
                tracing::warn!(
                    error.type = SharedError::CrashRecordsUnavailable.code(),
                    kind = ?error.kind(),
                    "the pending crash reports could not be read"
                );
                return;
            }
        };
        let (started, mut sent) = (Instant::now(), 0);
        for record in records {
            if started.elapsed() > PASS_BUDGET || !self.is_open() {
                break;
            }
            let Some(id) = self.installation_id() else { break };
            let Some(bytes) = report::envelope(report::event(&record, &id)) else { continue };
            // The switch is asked again just before the POST: "No enviar" may have come while the event was built.
            if !self.is_open() {
                break;
            }
            match post.post(bytes, POST_BUDGET) {
                Ok(()) => {
                    sent += 1;
                    if let Err(error) = store.remove(&record.id) {
                        tracing::warn!(
                            error.type = SharedError::CrashRecordsUnavailable.code(),
                            kind = ?error.kind(),
                            "a sent crash report could not be deleted; it will be sent again under the same id"
                        );
                    }
                }
                Err(PostError) => {
                    tracing::warn!(
                        error.type = SharedError::CrashReportNotSent.code(),
                        "a crash report could not be sent; it stays for the next start"
                    );
                    break;
                }
            }
        }
        if sent > 0 {
            tracing::info!(sent, "crash reports from a previous run were sent");
        }
    }

    /// Records a panic, from the panic hook. Never panics and never waits for a lock the panicking thread may hold: only the
    /// frames' addresses (the dying thread's) are taken here; a thread of its own resolves them and writes the record, and the
    /// hook waits for it for [`HOOK_WAIT`] at most. Nothing is made under `never`, and at most [`MAX_PER_RUN`] a run.
    pub fn record_panic(self: &Arc<Self>, panic: &Panicked<'_>) {
        if !self.is_open() || SENDING.with(Cell::get) || CAPTURING.swap(true, Ordering::SeqCst) {
            return;
        }
        let release = Release;
        if self.recorded.fetch_add(1, Ordering::SeqCst) >= MAX_PER_RUN {
            return;
        }
        let trace = backtrace::Backtrace::new_unresolved();
        let (literal, file) = (panic.literal.map(str::to_owned), panic.file.to_owned());
        let (formatted_len, line) = (panic.formatted_len, panic.line);
        let reporter = self.clone();
        let (done, finished) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("crash-capture".into()).spawn(move || {
            // Released before the hook is told, so that the next panic, right after this one, finds the capture free; a panic
            // in `write` releases it as it unwinds.
            let release = release;
            let _ = contained("record-panic", || reporter.write(literal.as_deref(), formatted_len, &file, line, trace));
            drop(release);
            let _ = done.send(());
        });
        // On a spawn that failed the closure, `release` with it, was dropped: the next panic may try again.
        if spawned.is_ok() {
            let _ = finished.recv_timeout(HOOK_WAIT);
        }
    }

    /// The capture thread's work.
    fn write(
        &self,
        literal: Option<&str>,
        formatted_len: usize,
        file: &str,
        line: u32,
        mut trace: backtrace::Backtrace,
    ) {
        let Some(store) = &self.store else { return };
        trace.resolve();
        let frames = trace::frames(&trace);
        let images = trace::holding(&self.images, &frames);
        let crashed = SystemTime::now();
        let record = CrashRecord::from_panic(
            Panic {
                resource: self.resource,
                at_ms: crashed
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(0)),
                literal,
                formatted_len,
                file,
                line,
                frames,
                images,
                tail: tail::snapshot(crashed),
            },
            &crate::logging::home(),
        );
        // A thread that panicked inside the store holds its lock for good: the record is dropped after a short wait. Nobody is told:
        // a log line from here could wait on the writer the dying thread holds.
        let _ = store.record(&record, || self.is_open());
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// Has the reporter send the handled errors of this process from now on, under its switch (call after `logging::init`, which makes
/// the layer that sees them, and after `Reporter::start`): the `error` events of the process's own targets (never the panic's, never
/// the page's) whose `error.type` is one of `codes`, which is the app's own list, built from its error enums (the codes of its own
/// crates; a code of a dependency, or any other value however well formed, is only counted: [`unreported_errors`]). At most
/// [`MAX_CODES`] are taken, once each. A reporter that cannot send (a build with no DSN, no place for its files) watches for nothing.
/// Never blocks, never panics.
pub fn report_handled_errors(reporter: Arc<Reporter>, codes: impl IntoIterator<Item = &'static str>) {
    let codes: Vec<&'static str> = codes.into_iter().collect();
    let _ = contained("handled-errors", || {
        if let Some(intake) = handled::intake() {
            handled::start(intake, reporter, codes);
        }
    });
}

/// Has the reporter record every panic of this process from now on, through the log's panic hook (call after `logging::init`).
/// Inside the hook nothing of the reporter's blocks or can abort: the frames are taken there (they are the panicking thread's),
/// the rest on a thread of its own, so that a panic or a stall in the reporter cannot hold the process up beyond [`HOOK_WAIT`].
pub fn report_panics(reporter: Arc<Reporter>) {
    crate::logging::observe_panics(move |panic| reporter.record_panic(panic));
}
