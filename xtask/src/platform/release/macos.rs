//! macOS: the Apple-Silicon app and its `.dmg`, signed with `codesign`, notarized with `notarytool`, checked with
//! `plutil` and `spctl`, stapled, and the native flows on the probe app.

use super::Os;
use crate::util::Channel;
use crate::Result;

#[path = "macos/app.rs"]
mod app;
#[path = "macos/memory.rs"]
mod memory;
#[path = "macos/probe.rs"]
mod probe;

pub struct Native;

impl Os for Native {
    fn build_app(signed: bool, channel: Channel) -> Result<()> {
        app::build_app(signed, channel)
    }

    fn check_app(signed: bool) -> Result<()> {
        app::check_app(signed)
    }

    fn first_open() -> Result<()> {
        app::first_open()
    }

    fn package(signed: bool) -> Result<()> {
        app::dmg(signed)
    }

    fn sign_package() -> Result<()> {
        app::sign_dmg()
    }

    fn checksums() -> Result<()> {
        app::checksums()
    }

    fn forget_secrets() -> Result<()> {
        app::forget_secrets()
    }

    fn package_screens() -> Result<()> {
        app::dmg_screens()
    }

    fn probe_app() -> Result<()> {
        probe::probe_app()
    }

    fn local_host() -> Result<()> {
        probe::local_host()
    }

    fn probe() -> Result<()> {
        probe::probe()
    }

    fn voice_models() -> Result<()> {
        probe::voice_models()
    }

    fn voice_memory() -> Result<()> {
        probe::voice_memory()
    }

    fn memory() -> Result<()> {
        probe::memory()
    }

    fn card() -> Result<()> {
        probe::card()
    }

    fn exit() -> Result<()> {
        probe::exit()
    }
}
