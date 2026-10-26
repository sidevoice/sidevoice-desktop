//! The release steps of the desktop app, one set of commands on every OS (`cargo xtask release <step>`): the app
//! built, checked and started as a release ships it, its installer package made, signed and checksummed. What each OS
//! does differently is in `release/{macos,windows,linux}.rs`; a step an OS has no use for says so and does nothing.

use crate::platform::host;
use std::path::PathBuf;

use crate::util::{ensure, read, require_ci, scratch_home, sleep, wait_for, work, Channel, Cmd};
use crate::Result;

/// A host's way to run each release step. Every method is one real difference between hosts; the default is that the
/// host has no such step.
trait Os {
    /// The app bundle, ad-hoc signed; `signed`: Developer ID signed, notarized and stapled by Tauri's bundler. Also
    /// its update artifact for a release or nightly `channel`.
    fn build_app(_signed: bool, _channel: Channel) -> Result<()> {
        Err(unsupported("app"))
    }

    /// What a first open depends on in the built app (`signed`: its signature, notarization and stapling too).
    fn check_app(_signed: bool) -> Result<()> {
        Err(unsupported("check-app"))
    }

    /// Whether the app can run off a runner with a scratch home (`HOME` redirected). A host whose settings and data are in
    /// folders no environment variable moves cannot: its first-open runs on a runner only.
    const SCRATCH_HOME: bool = false;

    /// The built app's binary, as `tauri build` leaves it on this host; `None` when the host has no first-open.
    fn app_binary() -> Option<PathBuf> {
        None
    }

    /// The log line (a regex) that says the bundled interface loaded, with the origin it is served from on this host.
    const INTERFACE_PAGE: &'static str = r"page \S+/voice/index\.html";

    /// Refuses a host where the app has already stored settings or webview data, which a first open would meet.
    fn require_fresh_profile() -> Result<()> {
        Ok(())
    }

    /// The installer package (macOS: the `.dmg` with the brand's window), verified, the app inside checked.
    fn package(_signed: bool) -> Result<()> {
        Err(unsupported("package"))
    }

    /// The package signed, notarized, stapled and checked as a download has it.
    fn sign_package() -> Result<()> {
        Err(unsupported("sign-package"))
    }

    /// `dist/SHA256SUMS.txt` for the package.
    fn checksums() -> Result<()> {
        Err(unsupported("checksums"))
    }

    /// Whatever a signing step cut short left (its keychain, certificate and key files).
    fn forget_secrets() -> Result<()> {
        Err(unsupported("forget-secrets"))
    }

    /// The package's window as the system shows it, light and dark (informational).
    fn package_screens() -> Result<()> {
        Err(unsupported("package-screens"))
    }
}

#[cfg_attr(target_os = "macos", path = "release/macos.rs")]
#[cfg_attr(target_os = "windows", path = "release/windows.rs")]
#[cfg_attr(target_os = "linux", path = "release/linux.rs")]
mod imp;

use imp::Native;

/// The refusal of a step this host does not have.
fn unsupported(step: &str) -> String {
    format!("`cargo xtask release {step}` is not available on {}", host::name())
}

/// `release app [--signed] [--channel C]`.
pub fn build_app(signed: bool, channel: Channel) -> Result<()> {
    Native::build_app(signed, channel)
}

/// `release check-app [--signed]`.
pub fn check_app(signed: bool) -> Result<()> {
    Native::check_app(signed)
}

/// How long the interface's controller has to come up, in seconds.
const READY_SECONDS: u64 = 90;
/// How long the app must then keep running, in seconds.
const SURVIVAL_SECONDS: f64 = 5.0;

/// `release first-open`: the app started with no settings, the way a person's first open is: the bundled interface
/// comes up, its controller attached to the web UI and reaching the app over IPC, and the app is still running a few
/// seconds later. Asking for the probe page changes nothing in a release. Off a runner it runs with a scratch home on a
/// host that can have one, and refuses on one that cannot (it would write the person's settings and data).
pub fn first_open() -> Result<()> {
    let Some(binary) = Native::app_binary() else { return Err(unsupported("first-open")) };
    let home = if Native::SCRATCH_HOME {
        scratch_home("first-open")?
    } else {
        require_ci("release first-open")?;
        None
    };
    // This check requires a fresh runner even when CI=true is set locally.
    Native::require_fresh_profile()?;
    let log = work("first-open")?.join("app.log");
    let mut app = Cmd::new(binary)
        .env("SIDEVOICE_DEBUG", "1")
        .env("SIDEVOICE_DEBUG_PAGE", "probe.html")
        .env_remove("SIDEVOICE_DATA_DIR");
    if let Some(home) = &home {
        app = app.env("HOME", home);
    }
    let mut app = app.spawn(&log)?;
    ensure(
        wait_for(&log, "ready: true", 0, READY_SECONDS),
        format!("The interface's controller was not ready within {READY_SECONDS} seconds:\n{}", read(&log)),
    )?;
    sleep(SURVIVAL_SECONDS);
    if !app.running() {
        return Err(format!("The app exited on its own:\n{}", read(&log)));
    }
    app.stop();
    let text = read(&log);
    eprintln!("{text}");
    let has = |pattern| crate::util::has(&text, pattern);
    ensure(has(Native::INTERFACE_PAGE), "the bundled interface did not load")?;
    ensure(!has("probe.html"), "a release ran a CI probe")?;
    ensure(has("ready: true"), "the interface's controller never came up")
}

/// `release package [--signed]`.
pub fn package(signed: bool) -> Result<()> {
    Native::package(signed)
}

/// `release sign-package`.
pub fn sign_package() -> Result<()> {
    Native::sign_package()
}

/// `release checksums`.
pub fn checksums() -> Result<()> {
    Native::checksums()
}

/// `release forget-secrets`.
pub fn forget_secrets() -> Result<()> {
    Native::forget_secrets()
}

/// `release package-screens`.
pub fn package_screens() -> Result<()> {
    Native::package_screens()
}
