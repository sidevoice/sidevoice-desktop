//! macOS: the flows on the probe app, the app's window server, Keychain and crash reports.

use super::Os;
use crate::Result;

#[path = "macos/flows.rs"]
mod flows;

pub struct Native;

impl Os for Native {
    fn probe_app() -> Result<()> {
        flows::probe_app()
    }

    fn local_host() -> Result<()> {
        flows::local_host()
    }

    fn probe() -> Result<()> {
        flows::probe()
    }

    fn voice_memory_log() -> Result<String> {
        flows::voice_memory_log()
    }

    fn memory_log() -> Result<String> {
        flows::memory_log()
    }

    fn card() -> Result<()> {
        flows::card()
    }

    fn exit() -> Result<()> {
        flows::exit()
    }
}
