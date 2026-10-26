//! Where tests make their scratch directories, and the one directory mode the portable test-file operations lack.
use std::path::{Path, PathBuf};

#[cfg(unix)]
#[path = "temp/unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "temp/windows.rs"]
mod imp;

pub use imp::Os as Temp;

pub trait TempArea {
    /// A directory whose children get paths short enough for a local socket, and which a private child is safe in.
    fn short_root() -> PathBuf;

    /// Makes `dir` writable by everybody, with deletion limited to each file's owner (the sticky bit, as `/tmp` is).
    /// Returns `false`, changing nothing, where the OS has no such mode.
    fn share_sticky(dir: &Path) -> bool;
}
