//! Stop requests through `ctrlc`: Ctrl+C everywhere, SIGTERM and SIGHUP on Unix.

use super::Interruption;
use crate::Result;

pub struct Host;

impl Interruption for Host {
    fn register(on_stop: fn()) -> Result<()> {
        ctrlc::set_handler(on_stop).map_err(|e| e.to_string())
    }
}
