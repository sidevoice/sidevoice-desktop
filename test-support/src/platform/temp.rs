//! Where tests make their scratch directories, and the one directory mode the portable test-file operations lack.
use std::path::{Path, PathBuf};

#[cfg(unix)]
#[path = "temp/unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "temp/windows.rs"]
mod imp;

use imp::Os;

/// The only parts of the scratch area that differ per OS.
trait Area {
    /// A directory whose children get paths short enough for a local socket, and which a private child is safe in.
    fn short_root() -> PathBuf {
        std::env::temp_dir()
    }

    /// Makes `dir` writable by everybody, with deletion limited to each file's owner. `false`, changing nothing, where
    /// the OS has no such mode.
    fn share_sticky(_dir: &Path) -> bool {
        false
    }
}

/// A directory whose children get paths short enough for a local socket, and which a private child is safe in.
pub fn short_root() -> PathBuf {
    Os::short_root()
}

/// Makes `dir` writable by everybody, with deletion limited to each file's owner (the sticky bit, as `/tmp` is).
/// Returns `false`, changing nothing, where the OS has no such mode.
pub fn share_sticky(dir: &Path) -> bool {
    Os::share_sticky(dir)
}
