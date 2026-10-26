//! macOS: the Apple-Silicon app and its `.dmg`, signed with `codesign`, notarized with `notarytool`, checked with
//! `plutil` and `spctl`, and stapled.

use std::path::PathBuf;

use super::Os;
use crate::platform::macos_app::binary;
use crate::util::Channel;
use crate::Result;

#[path = "macos/app.rs"]
mod app;

pub struct Native;

impl Os for Native {
    fn build_app(signed: bool, channel: Channel) -> Result<()> {
        app::build_app(signed, channel)
    }

    fn check_app(signed: bool) -> Result<()> {
        app::check_app(signed)
    }

    const SCRATCH_HOME: bool = true;

    const INTERFACE_PAGE: &'static str = r"page tauri://localhost/voice/index\.html";

    fn app_binary() -> Option<PathBuf> {
        Some(binary())
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
}
