//! What the first-open smoke test needs from the host: where the shipped app is, and whether the person's profile is
//! untouched.

use std::path::{Path, PathBuf};

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
