//! Hearing that the person (or the system) asks this process to stop.

use crate::Result;

/// Calls `on_stop` on a helper thread for every stop request from now on: Ctrl+C on every host, and SIGTERM and SIGHUP
/// on Unix (the `ctrlc` library supplies all of it). The caller owns what stopping means.
pub fn register(on_stop: fn()) -> Result<()> {
    ctrlc::set_handler(on_stop).map_err(|e| e.to_string())
}
