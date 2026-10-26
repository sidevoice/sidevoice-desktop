//! The fixture's only OS-specific operations.
use std::{io, path::Path, process::Command};
#[cfg(unix)]
#[path = "platform/unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "platform/windows.rs"]
mod imp;
pub mod registry;
pub use imp::Os as Host;
pub trait FixtureHost {
    /// Spawn a service outside the caller's process group/job.
    fn detach(command: &mut Command) -> io::Result<std::process::Child>;
    /// Remove filesystem endpoint directories; named pipes leave none.
    fn forget(_data: &Path) {}
}
