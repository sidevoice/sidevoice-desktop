//! The first-open smoke test (`windows first-open`): what it needs from the host (where the shipped app is, and
//! whether the person's profile is untouched) and the check itself.

use std::path::{Path, PathBuf};

use crate::platform::host::require_os;
use crate::util::{ensure, read, repo, require_ci, sleep, wait_for, work, Cmd};
use crate::Result;

/// A host's facts for starting the shipped app as a first-time user would.
pub trait FirstOpen {
    /// The app's binary under `repo`, as `tauri build` leaves it on this host.
    fn binary(repo: &Path) -> PathBuf;

    /// Refuses a host where the app has already stored settings or webview data: Tauri resolves them from the
    /// host's own profile folders, not from environment overrides, and a person's profile is never deleted. A host
    /// without a smoke test returns an error saying so.
    fn require_fresh_profile() -> Result<()>;
}

#[cfg(windows)]
#[path = "first_open/windows.rs"]
mod imp;
#[cfg(not(windows))]
#[path = "first_open/other.rs"]
mod imp;

pub use imp::Host;

/// `windows first-open`: the app started with no settings, on a runner: the bundled interface comes up in the real
/// WebView2, its controller attached to the web UI and reaching the app over IPC, and the app is still running a few
/// seconds later. Asking for the probe page changes nothing in a release.
pub fn run() -> Result<()> {
    require_os("windows", "windows first-open")?;
    // It writes the person's settings and data: a throwaway runner only.
    require_ci("windows first-open")?;
    // This check requires a fresh runner even when CI=true is set locally.
    Host::require_fresh_profile()?;
    let log = work("first-open")?.join("app.log");
    let mut app = Cmd::new(Host::binary(&repo()))
        .env("SIDEVOICE_DEBUG", "1")
        .env("SIDEVOICE_DEBUG_PAGE", "probe.html")
        .env_remove("SIDEVOICE_DATA_DIR")
        .spawn(&log)?;
    ensure(
        wait_for(&log, "ready: true", 0, 90),
        format!("The interface's controller was not ready within 90 seconds:\n{}", read(&log)),
    )?;
    sleep(5.0);
    if !app.running() {
        return Err(format!("The app exited on its own:\n{}", read(&log)));
    }
    app.stop();
    let text = read(&log);
    eprintln!("{text}");
    let has = |pattern| crate::util::has(&text, pattern);
    ensure(has(r"page \S+/voice/index\.html"), "the bundled interface did not load")?;
    ensure(!has("probe.html"), "a release ran a CI probe")?;
    ensure(has("ready: true"), "the interface's controller never came up")
}
