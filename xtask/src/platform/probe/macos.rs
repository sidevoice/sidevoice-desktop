//! macOS: the flows on the probe app, the app's window server, Keychain and crash reports.

use super::Os;
use crate::Result;

#[path = "macos/flows.rs"]
mod flows;
#[path = "macos/memory.rs"]
mod memory;

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

    fn voice_models() -> Result<()> {
        flows::voice_models()
    }

    fn voice_memory() -> Result<()> {
        flows::voice_memory()
    }

    fn memory() -> Result<()> {
        flows::memory()
    }

    fn card() -> Result<()> {
        flows::card()
    }

    fn exit() -> Result<()> {
        flows::exit()
    }
}
