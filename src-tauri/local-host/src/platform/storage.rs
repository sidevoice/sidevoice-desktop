//! The local-host storage boundary uses the pinned shared implementation on every host.
pub use sidevoice_platform::fs::{Access, Directory, HeldDirectory};
use sidevoice_platform::fs::{Files, Fs};
use std::{
    io,
    path::{Path, PathBuf},
};

/// Existing-only discovery and private creation. Held directories retain the shared no-follow,
/// owner, ancestry and single-name record checks for every read, write and removal.
pub trait TrustedStorage {
    /// Open an existing directory without creating it or repairing its permissions.
    fn open(path: &Path, access: Access) -> io::Result<HeldDirectory> {
        HeldDirectory::open(path, access)
    }
    /// Create missing private directories; refuse unsafe existing directories and ancestry.
    fn ensure_private_dir(path: &Path) -> io::Result<()> {
        Files::ensure_private_dir(path)
    }
    /// Resolve a recorded program through the shared executable and ancestry checks.
    fn trusted_program(path: &Path) -> io::Result<PathBuf> {
        Files::trusted_program(path)
    }
}

/// All hosts use the same maintained shared storage implementation; no local native copy.
pub struct Storage;
impl TrustedStorage for Storage {}
