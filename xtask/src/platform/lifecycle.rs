//! Hearing that the person (or the system) asks this process to stop.

use crate::Result;

/// A host's way to hear a stop request: Ctrl+C on every host, and the termination signals where the host has them.
pub trait Interruption {
    /// Calls `on_stop` on a helper thread for every stop request from now on. The caller owns what stopping means.
    /// A host that cannot register returns an error saying so.
    fn register(on_stop: fn()) -> Result<()>;
}

/// One maintained library (`ctrlc`) supplies the handler on every host, so there is one implementation, not one per OS.
#[path = "lifecycle/default.rs"]
mod imp;

pub use imp::Host;
