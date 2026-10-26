//! Removing the runtime directories the fixtures' endpoints leave behind.
use sidevoice_platform::ipc::{Address, Endpoint};
use std::path::{Path, PathBuf};

#[cfg(unix)]
#[path = "endpoint_cleanup/unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "endpoint_cleanup/windows.rs"]
mod imp;

use imp::Os;

/// The only part of endpoint cleanup that differs per OS.
trait Endpoints {
    /// Whether an endpoint is a file in a directory of its own (a Unix socket in the runtime directory). A named pipe
    /// has no directory, so by default there is nothing to remove.
    const LEAVE_DIRECTORIES: bool = false;
}

/// Removes the endpoint directories created for `data` outside it (the core's and the connector's).
pub fn forget(data: &Path) {
    if !Os::LEAVE_DIRECTORIES {
        return;
    }
    for endpoint in [Endpoint::new(&data.join("core"), "local"), Endpoint::new(data, "connector")] {
        let path = PathBuf::from(endpoint.to_string());
        if let Some(parent) = path.parent().filter(|p| !p.starts_with(data)) {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}
