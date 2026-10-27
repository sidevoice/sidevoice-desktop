//! Unix permission bits of a file.

use std::path::Path;

use crate::Result;

/// A host's way to read and set the permission bits of a file. A host without Unix permissions says so and changes
/// nothing.
trait Os {
    /// The permission bits of `path` (`stat -f %Lp`).
    fn mode(path: &Path) -> Result<u32>;

    /// Sets the permission bits of `path`.
    fn chmod(path: &Path, mode: u32) -> Result<()>;
}

#[cfg(unix)]
#[path = "file_mode/unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "file_mode/windows.rs"]
mod imp;

use imp::Native;

/// The permission bits of `path`.
pub fn mode(path: &Path) -> Result<u32> {
    Native::mode(path)
}

/// Sets the permission bits of `path`.
pub fn chmod(path: &Path, mode: u32) -> Result<()> {
    Native::chmod(path, mode)
}
