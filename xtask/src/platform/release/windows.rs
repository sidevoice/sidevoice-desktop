//! Windows: the `.exe` of `tauri build` started on a runner; the profile is the Known Folders `RoamingAppData` and
//! `LocalAppData`. Its installer comes from `cargo xtask bundle`.

use std::path::{Path, PathBuf};

use super::Os;
use crate::util::{ensure, read, repo, require_ci, sleep, wait_for, work, Cmd};
use crate::Result;

/// The app's identifier: the folder Tauri creates under each Known Folder.
const IDENTIFIER: &str = "dev.sidevoice.desktop";

pub struct Native;

/// The app's binary under `repo`, as `tauri build` leaves it.
fn binary(repo: &Path) -> PathBuf {
    repo.join("src-tauri/target/release/sidevoice-desktop.exe")
}

/// Refuses a host where the app has already stored settings or webview data: Tauri resolves them from the host's own
/// profile folders, not from environment overrides, and a person's profile is never deleted.
fn require_fresh_profile() -> Result<()> {
    // `dirs` resolves both through `SHGetKnownFolderPath`.
    for (name, root) in [("RoamingAppData", dirs::config_dir()), ("LocalAppData", dirs::data_local_dir())] {
        let root = root.ok_or_else(|| format!("Cannot resolve {name}"))?;
        let profile = root.join(IDENTIFIER);
        if profile.exists() {
            return Err(format!("release first-open requires a fresh runner: {} already exists", profile.display()));
        }
    }
    Ok(())
}

impl Os for Native {
    /// The app started with no settings, on a runner: the bundled interface comes up in the real WebView2, its
    /// controller attached to the web UI and reaching the app over IPC, and the app is still running a few seconds
    /// later. Asking for the probe page changes nothing in a release.
    fn first_open() -> Result<()> {
        // It writes the person's settings and data: a throwaway runner only.
        require_ci("release first-open")?;
        // This check requires a fresh runner even when CI=true is set locally.
        require_fresh_profile()?;
        let log = work("first-open")?.join("app.log");
        let mut app = Cmd::new(binary(&repo()))
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
}
