//! Crash reports (Sentry), shared by the desktop and the phone (decided with the operator, 2026-11-15; tracked in
//! sidevoice-desktop#523).
//!
//! What the app does when something fails is one setting the app owns, [`Mode`]: send automatically (the default, never
//! asked) or never. Under `auto` the app sends its own panics and the crash records of the machines it is connected to
//! ([`machine`]); under `never` nothing is initialised, fetched, acknowledged or sent, and what was queued is dropped
//! ([`Reporter::set_mode`]).
//!
//! What leaves is a closed field list ([`event`]): an event is built from a fixed set of fields and nothing else, and a
//! last filter (`before_send`) rebuilds it from that set again, so no SDK default, integration or scope can add a field.
//! Nothing that is not on the list is sent: no user, IP, request, breadcrumb, context, server name or module list.
//! Formatted panic messages are never sent, only a literal or the fixed marker (`docs/logging.md`, section 9).
//!
//! - [`settings`]: the mode and the random installation id (pseudonymous, rotated yearly), in one small local file.
//! - [`event`]: the closed field list and the two events built from it: the app's own panic, and a machine's record.
//! - [`transport`]: the gate every envelope passes (open only under `auto`, events only) and the POST that tells whether
//!   an event was delivered, which is what lets a machine's record be acknowledged only after a successful send.
//! - [`reporter`]: the client, its lifecycle and the two ways in.
//! - [`machine`]: the machines' records, fetched and acknowledged through the app's way to a machine.

pub mod event;
#[cfg(test)]
mod hardening_tests;
pub mod machine;
pub mod reporter;
pub mod settings;
mod tail;
#[cfg(test)]
mod tests;
pub mod transport;

use serde::{Deserialize, Serialize};
use std::time::Duration;

pub use machine::{Machine, MachineError, MachineRecord, Synced};
pub use reporter::{Config, Reporter, SendError};
pub use settings::Settings;
pub use transport::{Post, PostError};

/// What happens when something fails (Settings → Crash reports).
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

/// Runs `step` so that a panic in it ends there: the reporter is best-effort and never takes the app down. A panic is
/// logged (a fixed sentence and `error.type`; the step is a fixed name, the payload is never read) and `None` comes back.
pub(crate) fn contained<T>(step: &'static str, run: impl FnOnce() -> T) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(value) => Some(value),
        Err(_) => {
            tracing::warn!(
                error.type = crate::error::SharedError::CrashReporterFailed.code(),
                step,
                "a step of the crash reporter failed and was contained; the app goes on"
            );
            None
        }
    }
}

/// Only one panic is being handed to the reporter at a time, process-wide; a panic that comes while one is (another
/// thread's, or the reporter's own) is not reported.
static HANDING_OVER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How long the panic hook waits for the reporter in all: the POST's budget and the time to hear its outcome, plus a margin.
/// A panic inside the hook aborts the process and cannot be caught, so the reporter's work runs on a thread of its own and
/// the hook only waits, at most this long.
const HOOK_WAIT: Duration = Duration::from_millis(2600);

struct Release;

impl Drop for Release {
    fn drop(&mut self) {
        HANDING_OVER.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Has the reporter told of every panic of this process from now on, through the log's panic hook (call after
/// `logging::init`). Inside the hook nothing of the reporter's runs: the frames are taken there (they are the panicking
/// thread's), the rest on a thread of its own, so that a panic or a stall in the reporter cannot abort the process or hold
/// it up beyond [`HOOK_WAIT`].
pub fn report_panics(reporter: std::sync::Arc<Reporter>) {
    use std::sync::atomic::Ordering;
    crate::logging::observe_panics(move |literal, thread| {
        if !reporter.would_send()
            || HANDING_OVER.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err()
        {
            return;
        }
        let release = Release;
        let stacktrace = sentry::integrations::backtrace::current_stacktrace();
        let images = sentry::integrations::debug_images::debug_images();
        let (literal, thread, reporter) = (literal.map(str::to_owned), thread.to_owned(), reporter.clone());
        let (tell, hear) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("crash-report".into()).spawn(move || {
            let _release = release;
            reporter.capture_panic_with(literal.as_deref(), &thread, stacktrace, images);
            let _ = tell.send(());
        });
        // On a spawn that failed the closure, `release` with it, was dropped: the next panic may try again.
        if spawned.is_ok() {
            let _ = hear.recv_timeout(HOOK_WAIT);
        }
    });
}
