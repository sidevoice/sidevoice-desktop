//! Handled errors: the `error` events of our own targets, sent to Sentry under the crash-report switch (the logging guideline,
//! section 10), by the desktop and the phone alike.
//!
//! What is sent is decided here and by construction, not by filtering a bigger event afterwards:
//!
//! - **Which events.** `error` level, a target of the process's own (the list `logging::init` was given), never one of
//!   [`NOT_REPORTED`] (the panic hook's two and the page's `webview`: written out and checked first, and a target is matched whole
//!   or up to a `::`, so a lookalike such as `panic_x` is not ours either). And one of the app's own error codes: `error.type` must
//!   be one of the list the app gave [`report_handled_errors`](super::report_handled_errors) (built from its error enums), so a
//!   value of any other kind, however well formed (an address, a host name, a word somebody typed, a dependency's code), is never
//!   sent. An event without such a code is counted ([`unreported`]) and nothing else.
//! - **What of an event.** The layer reads exactly one field of it, `error.type`, and only as a string; what it keeps is the matching
//!   `&'static str` of the app's list, never a byte of the event's own text. The message and every other field value are never
//!   looked at, so nothing can leak from them. The place is the callsite's own file and line, compiled into the binary, as a short
//!   relative path or `unknown`. The sent event ([`event`]) is the release, the five tags of [`TAGS`](super::report::TAGS) and one
//!   exception with that type and that one frame: no message, breadcrumb, extra, context, user or host name.
//! - **How often.** One event per `error.type` per day (UTC) per install: the same type again that day is dropped. The day's list
//!   lives in memory and in `handled-errors.json` beside the setting (read at start and written after a send; only codes of the
//!   app's list are read back from it); a different day, a clock that went back or a file that says another day all mean a new
//!   day. The list of codes is closed and holds at most [`MAX_CODES`], so neither a storm of codes nor a bad file can make the day's
//!   sends unbounded.
//! - **Under the switch.** The layer asks it before queueing: "never", a setting that could not be read, or a reporter that cannot
//!   send queue nothing (`Reporter::is_open`, an atomic). The sender asks it again, and a third time just before the POST. Choosing
//!   "No enviar" erases the day's list with the records and the install id (`Reporter::erase`).
//! - **Never breaks the app.** The layer does no network, reads no file and never waits: for an `error` event with a code of the
//!   list it `try_send`s into a queue of [`QUEUE`] (a full queue drops the event). The POST is on a thread of the reporter's own
//!   ([`reporter_thread`]), one at a time, [`POST_BUDGET`] each, every event inside `contained`, and after a failed send (or an install
//!   id that cannot be written) it waits [`QUIET`] before trying again (an error that comes with the network down comes in storms). That thread never feeds itself:
//!   the layer ignores events of a reporter thread, and what the reporter logs is `warn`.

use std::borrow::Cow;
use std::cell::Cell;
use std::fmt::Debug;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sentry_types::protocol::v7::{Event, Exception, Frame, Level, Stacktrace, Values};
use sentry_types::Uuid;
use serde::Serialize;
use tracing::field::{Field, Visit};
use tracing::{Event as Logged, Metadata, Subscriber};
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::{Context as Scope, Layer};
use tracing_subscriber::registry::LookupSpan;

use super::record::{random_uuid, relative};
use super::report::{self, PostError};
use super::settings::{read_small, write_atomic};
use super::tail::{kept, under};
use super::{contained, reporter_thread, Reporter, NOT_REPORTED, POST_BUDGET, SENDING};
use crate::error::SharedError;
use crate::logging::Resource;

/// The most codes an app may list: what bounds the types sent in a day, and the day's file (which must be readable, `read_small`).
pub const MAX_CODES: usize = 64;
/// How many events wait for the sender.
const QUEUE: usize = 16;
/// How long the sender leaves the network alone after a failed send.
const QUIET: Duration = Duration::from_secs(10 * 60);
/// The day's list, a file of the app's own directory beside the setting.
const CAP: &str = "handled-errors.json";

/// A handled error, as much of it as may leave: its code and the place of the `error!` that logged it.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Handled {
    pub kind: &'static str,
    pub file: String,
    pub line: u32,
}

/// What joins the layer, which is made with the process's subscriber, to the sender, which starts with the app's reporter: the
/// queue, the count of what is not the app's, and the app's answers once it has bound them.
pub(super) struct Intake {
    queue: SyncSender<Handled>,
    waiting: Mutex<Option<Receiver<Handled>>>,
    pub(super) unreported: AtomicU64,
    /// Set once, when the reporter takes over: until then the switch is not known and nothing is queued.
    source: OnceLock<Source>,
}

/// What the app answers for: its error codes, and whether its switch is on (`Reporter::is_open`).
struct Source {
    codes: Vec<&'static str>,
    open: Box<dyn Fn() -> bool + Send + Sync>,
}

impl Intake {
    pub(super) fn new(capacity: usize) -> Intake {
        let (queue, waiting) = sync_channel(capacity);
        Intake { queue, waiting: Mutex::new(Some(waiting)), unreported: AtomicU64::new(0), source: OnceLock::new() }
    }

    /// Has the layer ask `reporter` for its switch and `codes` for what is the app's.
    fn bind(&self, reporter: Arc<Reporter>, codes: Vec<&'static str>) {
        let _ = self.source.set(Source { codes, open: Box::new(move || reporter.is_open()) });
    }
}

/// The process's: the layer of `logging::init` is made over it, `report_handled_errors` starts its sender.
static INTAKE: OnceLock<Intake> = OnceLock::new();

pub(super) fn intake() -> Option<&'static Intake> {
    INTAKE.get()
}

/// How many `error` events of ours had no `error.type` of the app's own codes (none, not a string, or any other value) since the
/// reporter started watching for them in this process. They are not sent; this is all that is kept of them.
pub fn unreported() -> u64 {
    INTAKE.get().map_or(0, |intake| intake.unreported.load(Ordering::Relaxed))
}

/// The layer of the process, over `own` (its targets, `logging::init`'s): it queues nothing until the app's reporter has started it
/// ([`report_handled_errors`](super::report_handled_errors)).
pub fn layer<S>(own: &[&str], in_hook: fn() -> bool) -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    queueing(INTAKE.get_or_init(|| Intake::new(QUEUE)), own, in_hook)
}

/// The layer, with its filter: events at `error` of `own`'s targets, never [`NOT_REPORTED`]. `in_hook` says whether this thread is
/// inside the panic hook.
pub(super) fn queueing<S>(intake: &'static Intake, own: &[&str], in_hook: fn() -> bool) -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let targets = kept(own);
    Queueing { intake, in_hook }.with_filter(filter_fn(move |meta: &Metadata<'_>| handled(meta, &targets)))
}

fn handled(meta: &Metadata<'_>, own: &[String]) -> bool {
    let target = meta.target();
    meta.is_event()
        && *meta.level() == tracing::Level::ERROR
        && !NOT_REPORTED.iter().any(|name| under(target, name))
        && own.iter().any(|name| under(target, name))
}

/// Reads one field of an event, `error.type`, and only when it is a string that is one of the app's codes.
struct ErrorType<'a> {
    codes: &'a [&'static str],
    found: Option<&'static str>,
}

impl Visit for ErrorType<'_> {
    /// What comes back is the list's own `&'static str`, so nothing of the event's text is kept.
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "error.type" {
            self.found = self.codes.iter().copied().find(|code| *code == value);
        }
    }

    /// Every other field, and any other way of recording `error.type`, is not looked at.
    fn record_debug(&mut self, _: &Field, _: &dyn Debug) {}
}

struct Queueing {
    intake: &'static Intake,
    /// Whether this thread is inside the panic hook: nothing is done there.
    in_hook: fn() -> bool,
}

impl<S: Subscriber> Layer<S> for Queueing {
    fn on_event(&self, event: &Logged<'_>, _: Scope<'_, S>) {
        // The reporter's own threads are never a source (no loop), nor is the panic hook.
        if (self.in_hook)() || SENDING.with(Cell::get) {
            return;
        }
        // Before the reporter has taken over the switch is not known: nothing is queued.
        let Some(source) = self.intake.source.get() else { return };
        let mut field = ErrorType { codes: &source.codes, found: None };
        event.record(&mut field);
        let Some(kind) = field.found else {
            self.intake.unreported.fetch_add(1, Ordering::Relaxed);
            return;
        };
        // "No enviar", a setting that could not be read, a build that cannot send: nothing is queued.
        if !(source.open)() {
            return;
        }
        let meta = event.metadata();
        // A full queue drops the event: the logging thread never waits.
        let _ = self.intake.queue.try_send(Handled {
            kind,
            file: relative(meta.file().unwrap_or("")),
            line: meta.line().unwrap_or(0),
        });
    }
}

/// The event of a handled error, from its closed field list: a random Sentry `event_id`, the app's version as the release, the five
/// tags of [`TAGS`](super::report::TAGS) and one exception, the code, with one frame (the place). `server_name` is unset; nothing
/// else is.
pub(super) fn event(resource: &Resource, error: &Handled, installation_id: &str) -> Event<'static> {
    let frame = Frame { filename: Some(error.file.clone()), lineno: Some(u64::from(error.line)), ..Default::default() };
    Event {
        event_id: Uuid::parse_str(&random_uuid()).unwrap_or_else(|_| Uuid::nil()),
        level: Level::Error,
        platform: Cow::Borrowed("native"),
        // Never the host name: said, not assumed (no client exists to fill it in).
        server_name: None,
        timestamp: SystemTime::now(),
        release: Some(report::release(resource.service_name, resource.service_version)),
        tags: report::tags(
            resource.service_name,
            resource.os_type,
            resource.os_name,
            resource.host_arch,
            installation_id,
        ),
        exception: Values::from(vec![Exception {
            ty: error.kind.to_owned(),
            value: None,
            stacktrace: Stacktrace::from_frames_reversed(vec![frame]),
            ..Default::default()
        }]),
        ..Default::default()
    }
}

/// The UTC day of `now`, as days since the Unix epoch (a clock before it is day 0).
pub(super) fn day_of(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs() / (24 * 60 * 60))
}

/// The types sent today, in memory and in a file beside the setting. Any other day than today's, earlier or later, is a new day.
pub(super) struct Cap {
    /// Empty when the app has no directory: nothing is kept then, nothing is erased.
    path: PathBuf,
    day: Option<u64>,
    sent: Vec<&'static str>,
}

/// The file's shape when written.
#[derive(Serialize)]
struct Kept<'a> {
    day: u64,
    sent: &'a [&'static str],
}

impl Cap {
    /// The list of the app whose own directory is `dir`; empty if there is none.
    pub(super) fn at(dir: PathBuf) -> Cap {
        let path = if dir.as_os_str().is_empty() { PathBuf::new() } else { dir.join(CAP) };
        Cap { path, day: None, sent: Vec::new() }
    }

    /// What the file says about `today`: nothing for another day, a file that is not ours or cannot be read, and only the codes of
    /// `codes`, once each, otherwise. Read back loosely: a value that is not a code of the app's is dropped, not the whole file.
    pub(super) fn load(&mut self, codes: &[&'static str], today: u64) {
        self.day = None;
        self.sent.clear();
        let Ok(Some(bytes)) = read_small(&self.path) else { return };
        let Ok(file) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return };
        if file["day"].as_u64() != Some(today) {
            return;
        }
        self.day = Some(today);
        let listed = file["sent"].as_array().map_or(&[][..], Vec::as_slice);
        for kind in listed
            .iter()
            .filter_map(serde_json::Value::as_str)
            .filter_map(|text| codes.iter().copied().find(|code| *code == text))
        {
            if !self.sent.contains(&kind) {
                self.sent.push(kind);
            }
        }
    }

    /// Starts today's list if the one held is for another day.
    fn enter(&mut self, today: u64) {
        if self.day != Some(today) {
            self.day = Some(today);
            self.sent.clear();
        }
    }

    /// Whether `kind` may be sent on `today`.
    pub(super) fn permits(&mut self, kind: &'static str, today: u64) -> bool {
        self.enter(today);
        !self.sent.contains(&kind)
    }

    /// Records that `kind` was sent on `today` (after `permits` said it might be) and keeps the list.
    pub(super) fn mark(&mut self, kind: &'static str, today: u64) -> io::Result<()> {
        self.enter(today);
        if !self.sent.contains(&kind) {
            self.sent.push(kind);
        }
        let bytes = serde_json::to_vec(&Kept { day: today, sent: &self.sent }).map_err(io::Error::other)?;
        write_atomic(&self.path, &bytes)
    }

    /// Forgets the day's list and deletes its file.
    pub(super) fn erase(&mut self) -> io::Result<()> {
        self.day = None;
        self.sent.clear();
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        match std::fs::remove_file(&self.path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

/// Why a handled error was or was not sent.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Sent,
    /// The switch is off, or cannot be read, or the reporter cannot send.
    Off,
    /// Its type was already sent today.
    Capped,
    /// The network failed a moment ago.
    Quiet,
    /// Not sent now: no install id, or the POST failed.
    Unsent,
}

/// The sender's state: one event at a time, under the switch, the day's cap and the quiet after a failure.
#[derive(Default)]
pub(super) struct Worker {
    quiet_until: Option<Instant>,
}

impl Worker {
    /// Sends `error` unless the switch is off, its type is capped on `today` or the network failed a moment ago. The type is
    /// counted as sent only once Sentry took the event, so an error that came with the network down is reported when it comes
    /// again.
    pub(super) fn handle(&mut self, reporter: &Reporter, error: &Handled, today: u64) -> Outcome {
        let Some(post) = reporter.post.as_ref().filter(|_| reporter.is_open()) else { return Outcome::Off };
        if self.quiet_until.is_some_and(|until| Instant::now() < until) {
            return Outcome::Quiet;
        }
        if !permits(reporter, error.kind, today) {
            return Outcome::Capped;
        }
        let Some(id) = reporter.installation_id() else {
            // None under "No enviar" (the switch went off meanwhile) is not a failure; otherwise the setting's file cannot be
            // written, and every error would say so again: it is left alone for a while, like a network that failed.
            if reporter.is_open() {
                self.quiet_until = Some(Instant::now() + QUIET);
                tracing::warn!(
                    error.type = SharedError::CrashSettingsNotKept.code(),
                    "the install id could not be had, so handled errors are not reported for a while"
                );
            }
            return Outcome::Unsent;
        };
        let Some(bytes) = report::envelope(event(&reporter.resource, error, &id)) else { return Outcome::Unsent };
        // The switch is asked again just before the POST: "No enviar" may have come while the event was built.
        if !reporter.is_open() {
            return Outcome::Off;
        }
        match post.post(bytes, POST_BUDGET) {
            Ok(()) => {
                keep(reporter, error.kind, today);
                Outcome::Sent
            }
            Err(PostError) => {
                self.quiet_until = Some(Instant::now() + QUIET);
                tracing::warn!(
                    error.type = SharedError::CrashReportNotSent.code(),
                    "a handled error could not be sent; none is tried again for a while"
                );
                Outcome::Unsent
            }
        }
    }
}

fn permits(reporter: &Reporter, kind: &'static str, today: u64) -> bool {
    reporter.handled.lock().unwrap_or_else(PoisonError::into_inner).permits(kind, today)
}

/// Counts `kind` as sent today and writes the day's list. A failure is one line and the list holds in memory until the app ends.
///
/// The switch is asked under the lock the erase of a "No enviar" takes after closing it: what is not written here because the
/// switch went off is not there to erase, and what was written before is erased, so no list is left for an app set not to send.
fn keep(reporter: &Reporter, kind: &'static str, today: u64) {
    let mut cap = reporter.handled.lock().unwrap_or_else(PoisonError::into_inner);
    if !reporter.is_open() {
        return;
    }
    if let Err(error) = cap.mark(kind, today) {
        tracing::warn!(
            error.type = SharedError::CrashSettingsNotKept.code(),
            kind = ?error.kind(),
            "the day's list of reported errors could not be saved; it holds until the app stops"
        );
    }
}

/// Starts the sender, once, when the app's reporter takes over: it takes what the layer queues from now on. A reporter that cannot
/// send (a build with no DSN, no place for its files) watches for nothing: the layer is never bound, so it queues nothing.
pub(super) fn start(intake: &'static Intake, reporter: Arc<Reporter>, codes: Vec<&'static str>) {
    if reporter.post.is_none() || reporter.store.is_none() {
        return;
    }
    let mut codes = codes.into_iter().fold(Vec::new(), |mut unique: Vec<&'static str>, code| {
        if !unique.contains(&code) {
            unique.push(code);
        }
        unique
    });
    codes.truncate(MAX_CODES);
    let Some(waiting) = intake.waiting.lock().unwrap_or_else(PoisonError::into_inner).take() else { return };
    reporter.handled.lock().unwrap_or_else(PoisonError::into_inner).load(&codes, day_of(SystemTime::now()));
    let sender = reporter.clone();
    if reporter_thread("crash-handled", move || run(&sender, &waiting)).is_err() {
        tracing::warn!(
            error.type = SharedError::CrashReporterFailed.code(),
            "the crash reporter could not start its thread for handled errors; none is sent in this run"
        );
        return;
    }
    intake.bind(reporter, codes);
}

/// The sender's loop: it ends only if the queue is gone, and an event that makes it panic costs that event.
fn run(reporter: &Reporter, waiting: &Receiver<Handled>) {
    let mut worker = Worker::default();
    while let Ok(error) = waiting.recv() {
        let _ = contained("handled-error", || worker.handle(reporter, &error, day_of(SystemTime::now())));
    }
}

#[cfg(test)]
impl Intake {
    /// The queue's receiving end, with the app's answers bound, for a test that drives the sender itself.
    pub(super) fn take_for_test(&self, reporter: Arc<Reporter>, codes: Vec<&'static str>) -> Receiver<Handled> {
        self.bind(reporter, codes);
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner).take().expect("the queue is there")
    }

    /// The codes the sender was started with, `None` while it has not been.
    pub(super) fn codes(&self) -> Option<&[&'static str]> {
        self.source.get().map(|source| source.codes.as_slice())
    }
}

#[cfg(test)]
impl Cap {
    /// The types held for today.
    pub(super) fn sent(&self) -> &[&'static str] {
        &self.sent
    }
}
