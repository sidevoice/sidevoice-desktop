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
pub mod machine;
pub mod reporter;
pub mod settings;
mod tail;
#[cfg(test)]
mod tests;
pub mod transport;

use serde::{Deserialize, Serialize};

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

/// Has the reporter told of every panic of this process from now on, through the log's panic hook (call after
/// `logging::init`).
pub fn report_panics(reporter: std::sync::Arc<Reporter>) {
    crate::logging::observe_panics(move |literal, thread| reporter.capture_panic(literal, thread));
}
