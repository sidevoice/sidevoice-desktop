//! `windows …`: the Windows app, started as the installer ships it. Its tools are the same as `macos`' (macos.rs), for the
//! one thing that holds on every OS: it starts.

use std::path::PathBuf;

use crate::util::{ensure, read, repo, require_ci, sleep, wait_for, work, Cmd};
use crate::Result;

/// The app's binary as `tauri build` leaves it (`cargo xtask bundle` on Windows).
fn binary() -> PathBuf {
    repo().join("src-tauri/target/release/sidevoice-desktop.exe")
}

/// `windows first-open`: the app started with no settings, on a runner: the bundled interface comes up in the real
/// WebView2, its controller attached to the web UI and reaching the app over IPC, and the app is still running a few
/// seconds later. Asking for the probe page changes nothing in a release.
pub fn first_open() -> Result<()> {
    // It writes the person's settings and data: a throwaway runner only.
    require_ci("windows first-open")?;
    let log = work("first-open")?.join("app.log");
    let mut app = Cmd::new(binary())
        .env("SIDEVOICE_DEBUG", "1")
        .env("SIDEVOICE_DEBUG_PAGE", "probe.html")
        .env_remove("SIDEVOICE_DATA_DIR")
        .spawn(&log)?;
    wait_for(&log, "ready: true", 0, 90);
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
