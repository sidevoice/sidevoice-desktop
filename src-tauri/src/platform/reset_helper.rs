//! Helper-mode dispatch. The authenticated handoff is independent of the existing cleanup transaction.

/// Runs before application initialization; no caller selects the host or supplies cleanup paths.
pub trait ResetHelper {
    /// Authenticate, observe the parent when supported, and await committed handoff before cleanup.
    /// Windows and Linux complete the same authenticated wait and release admission without cleanup;
    /// reset UI and the cleanup transaction remain unavailable on those hosts.
    fn run();
}

#[cfg(target_os = "macos")]
#[path = "reset_helper/macos.rs"]
mod native;
#[cfg(target_os = "windows")]
#[path = "reset_helper/windows.rs"]
mod native;
#[cfg(target_os = "linux")]
#[path = "reset_helper/linux.rs"]
mod native;

pub use native::DesktopResetHelper;
