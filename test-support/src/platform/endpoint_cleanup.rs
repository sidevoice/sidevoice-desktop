//! Removing the runtime directories the fixtures' endpoints leave behind.
use std::path::Path;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::Os as Cleanup;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::Os as Cleanup;

pub trait EndpointCleanup {
    /// Remove filesystem endpoint directories created for `data`; named pipes leave none.
    fn forget(data: &Path);
}
