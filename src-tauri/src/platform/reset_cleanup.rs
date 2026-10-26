//! The native pieces of the desktop reset that are neither owned directories (`owned_removal`) nor browser storage
//! (`webview_data`): whose home the cleanup runs in, the app's native preferences, and the app itself. The reset
//! transaction in `data.rs` keeps the sequencing, the journal and report, and the relaunch decision; it only asks
//! these operations for their result. A host without one of these capabilities keeps the documented default.

use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::NativeResetCleanup;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::NativeResetCleanup;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::NativeResetCleanup;

pub trait ResetCleanup {
    /// The home of the account running the app, as the OS account database records it (never `HOME`), or `None`
    /// where reset cleanup has no home to work in. Cleanup runs only when this equals the inherited `HOME`.
    fn account_home() -> Option<PathBuf> {
        None
    }

    /// Remove the app's native preferences, kept outside its owned directories. `Err` is the report's failure key
    /// for the `preferences` step; a host with no such store reports `unavailable`.
    fn remove_preferences() -> Result<(), &'static str> {
        Err("unavailable")
    }

    /// The installed app this run may remove on uninstall, checked again right before removal; a host that cannot
    /// remove its own installation refuses with `reset.app-unremovable`.
    fn removable_app() -> Result<PathBuf, &'static str> {
        Err("reset.app-unremovable")
    }

    /// Move `app` (from [`removable_app`](Self::removable_app)) where the person can restore it from (the Trash).
    /// A host without that refuses with `reset.app-trash-failed`, and the transaction keeps and relaunches the app.
    fn trash(_app: &Path) -> Result<(), &'static str> {
        Err("reset.app-trash-failed")
    }
}
