//! Build tooling for the desktop app, run as `cargo xtask <command>` (alias in `.cargo/config.toml`). CI runs these,
//! one step each (`.github/workflows`); each runs the same on a person's machine.
//!
//! - `icons`: draw the icons, tray marks and .dmg window from the brand files (`npm run icons`) and fail if they are
//!   not what is committed.
//! - `dist`: build this host's installers into `target/dist/`, as a release ships them. On macOS also check the app
//!   (Info.plist, the ad-hoc signature with the hardened runtime, the entitlements, the icon, arm64, no CI probe
//!   inside), start it twice (first open, and with a target), and make and check the brand's .dmg.
//! - `smoke` (macOS): the native flows of the app, in a build with the `probe` feature (never shipped): the engine's
//!   Kokoro → Whisper round trip, the local host from the bundled page, the probe page (engine, headset), the call
//!   controls card over a full-screen editor, and selecting models through the vendored room.
//! - `isolation` (Linux, CI): the local host with two users: a second uid cannot open the core's socket, link as a
//!   connector, or use the proxy.
//! - `manifest DIR [--tag vX.Y.Z]`: check that DIR holds every target's installer, give them their published names
//!   (`nightly` without a tag), and write `SHA256SUMS`; with a tag, the app's version must be that release's.
//! - `publish DIR TAG`: attach every file in DIR to the release TAG (for `nightly`, move the tag here first and drop
//!   older assets), download them back and check them against `SHA256SUMS`, and publish.
//!
//! The commands that start the app run it with a scratch `HOME`, so they never touch this machine's own settings,
//! models or pairing. `isolation` adds a user and is refused outside CI.

mod dist;
mod icons;
mod isolation;
#[cfg(target_os = "macos")]
mod macos;
mod manifest;
mod publish;
#[cfg(target_os = "macos")]
mod smoke;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod util;

use std::path::Path;

pub(crate) type Result<T> = std::result::Result<T, String>;

const USAGE: &str =
    "usage: cargo xtask icons | dist | smoke | isolation | manifest DIR [--tag vX.Y.Z] | publish DIR TAG";

/// The native flows run on macOS only: the probe build, WKWebView and the window server are the point.
fn smoke() -> Result<()> {
    #[cfg(target_os = "macos")]
    return smoke::smoke();
    #[cfg(not(target_os = "macos"))]
    Err("`cargo xtask smoke` runs on macOS".to_owned())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["icons"] => icons::icons(),
        ["dist"] => dist::dist(),
        ["smoke"] => smoke(),
        ["isolation"] => isolation::isolation(),
        ["manifest", dir] => manifest::manifest(Path::new(dir), None),
        ["manifest", dir, "--tag", tag] => manifest::manifest(Path::new(dir), Some(tag)),
        ["publish", dir, tag] => publish::publish(Path::new(dir), tag),
        _ => Err(USAGE.to_owned()),
    };
    if let Err(error) = result {
        eprintln!("xtask: {error}");
        std::process::exit(1);
    }
}
