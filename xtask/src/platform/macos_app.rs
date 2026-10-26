//! What the macOS app's steps share: where the built app is, its data, how it is started, and what a build must not
//! read from the environment.

use std::path::{Path, PathBuf};

use crate::util::{fresh_dir, in_ci, repo, Cmd};
use crate::Result;

pub fn release_target() -> &'static sidevoice_platform::target::ReleaseTarget {
    sidevoice_platform::target::for_platform("macos", "aarch64").expect("published macOS target")
}
/// The app's identifier (tauri.conf.json): its directory under Application Support.
pub const IDENTIFIER: &str = "dev.sidevoice.desktop";
/// Where the stand-in node (test/fixtures/fake-node.py) listens.
pub const FAKE_NODE_PORT: &str = "8768";

/// The bundle every `release` command works on, as `tauri build --target aarch64-apple-darwin` leaves it.
pub fn app() -> PathBuf {
    repo().join("src-tauri/target").join(release_target().triple).join("release/bundle/macos/Sidevoice.app")
}

/// The app's binary in [`app`].
pub fn binary() -> PathBuf {
    app().join("Contents/MacOS/sidevoice-desktop")
}

/// The app's settings and data under the person's home (`~/Library/Application Support/dev.sidevoice.desktop`).
pub fn app_data(home: &Path) -> PathBuf {
    home.join("Library/Application Support").join(IDENTIFIER)
}

pub fn home() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| "HOME is not set".into())
}

/// The app's binary, its diagnostics on (`SIDEVOICE_DEBUG=1`), with `env`.
pub fn app_command(env: &[(&str, &str)]) -> Cmd {
    let mut cmd = Cmd::new(binary()).env("SIDEVOICE_DEBUG", "1");
    for (key, value) in env {
        cmd = cmd.env(key, value);
    }
    cmd
}

/// The stand-in node (test/fixtures/fake-node.py) on loopback, logging every request with its Origin.
pub fn fake_node(log: &Path) -> Result<crate::util::Background> {
    Cmd::new("python3").args(["-u", "test/fixtures/fake-node.py", FAKE_NODE_PORT]).spawn(log)
}

/// The home a step starts the app with. On a runner (`CI=true`): the person's, the app's settings and data as a
/// fresh machine has them, as the YAML did. Elsewhere: a scratch home of the step's own (`/tmp/svx-<step>`, short: a
/// Unix socket's path is at most 104 bytes), made empty first, with `SIDEVOICE_DATA_DIR` unset, so that a laptop's own
/// settings, pairing, local core and models are never touched. (The app's provider keys are in the login keychain,
/// which a home does not change: `probe` expects none there, as on a runner.)
pub struct AppHome {
    pub home: PathBuf,
    scratch: bool,
}

impl AppHome {
    pub fn new(step: &str) -> Result<AppHome> {
        if in_ci() {
            let home = home()?;
            return Ok(AppHome { home, scratch: false });
        }
        let home = PathBuf::from("/tmp").join(format!("svx-{step}"));
        fresh_dir(&home)?;
        eprintln!("(off a runner: the app runs with HOME={}, this machine's own data untouched)", home.display());
        Ok(AppHome { home, scratch: true })
    }

    /// The app's binary with this home, its diagnostics on, with `env`.
    pub fn app(&self, env: &[(&str, &str)]) -> Cmd {
        let cmd = app_command(env);
        if self.scratch {
            cmd.env("HOME", &self.home).env_remove("SIDEVOICE_DATA_DIR")
        } else {
            cmd
        }
    }
}

/// The signing and notarization variables a build would otherwise read: an unsigned or probe build never signs with
/// the Developer ID, whatever this machine's environment holds (tauri.conf.json's ad-hoc identity then applies).
pub const SIGNING_ENV: &[&str] = &[
    "APPLE_CERTIFICATE",
    "APPLE_CERTIFICATE_PASSWORD",
    "APPLE_SIGNING_IDENTITY",
    "APPLE_ID",
    "APPLE_PASSWORD",
    "APPLE_TEAM_ID",
    "APPLE_API_KEY",
    "APPLE_API_ISSUER",
    "APPLE_API_KEY_PATH",
    "APPLE_API_KEY_P8",
];

/// `cmd` without [`SIGNING_ENV`].
pub fn unsigned(cmd: Cmd) -> Cmd {
    SIGNING_ENV.iter().fold(cmd, |cmd, name| cmd.env_remove(name))
}

/// What only the probe build carries (src-tauri/src/probe.rs): never in a release app.
pub const PROBE_ONLY: &[&str] = &[
    "SIDEVOICE_DEBUG_PAGE",
    "probe-engine",
    "probe-card",
    "SIDEVOICE_DEBUG_LOCAL_HOST_FLOW",
    "local-host-flow",
    "SIDEVOICE_DEBUG_EXIT",
];
