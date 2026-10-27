//! Adversarial storage fixtures. Assertions use security properties, never native permission APIs.
pub use sidevoice_platform::fs::Others;
use sidevoice_platform::fs::{Files, Fs, TestFiles};
use std::{
    fs,
    path::{Path, PathBuf},
};
#[cfg(unix)]
#[path = "storage/unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "storage/windows.rs"]
mod imp;
pub use imp::NativeStorageFixtures as StorageFixtures;

/// How the host prevents a directory replacement while its trusted handle is held.
pub enum Replacement {
    /// The pathname moved; later access must refuse the changed ancestry.
    Replaced,
    /// The host refused the rename while the handle was held; the original remains usable.
    Prevented,
}

/// Fixture mutations, with the permission and link implementation supplied by the pinned shared crate.
pub trait StorageFixture {
    /// Whether running a file is a permission on it (Unix: the execute bits), so that one without it is refused.
    const EXECUTION_IS_A_PERMISSION: bool;
    /// Make a scratch path private to its owner.
    fn private(path: &Path) {
        Files::shut_others_out(path);
    }
    /// Permit other users to read a file or traverse/list a directory.
    fn readable(path: &Path) {
        Files::let_others_read(path);
    }
    /// Permit other users to modify a file or replace directory children.
    fn writable(path: &Path) {
        Files::let_others_change(path);
    }
    /// Create a hostile link without following it.
    fn symlink(target: impl AsRef<Path>, link: impl AsRef<Path>) {
        Files::symlink(target, link);
    }
    /// Make a fixture payload executable. The shared implementation handles host conventions.
    fn executable(path: &Path) {
        Files::make_executable(path);
    }
    /// Read the actual access policy independently of a record read.
    fn others(path: &Path) -> Others {
        Files::others_of(&Files::open_read(path).unwrap()).unwrap()
    }
    /// Check ownership, private access, regular type and a single name on the opened file.
    fn assert_private(path: &Path) {
        Files::check_private(&Files::open_read(path).unwrap()).unwrap();
    }
    /// Assert the file remains non-executable where execution is an access permission.
    fn assert_not_executable(path: &Path);
    /// Exercise every native way a directory can violate private access.
    fn private_directory_violations(path: &Path, check: impl FnMut());
    /// Exercise native accepted and refused record grants; `check` receives whether the grant is trusted.
    fn record_access_cases(path: &Path, check: impl FnMut(bool));
    /// A non-regular record, including one that could block a naive reader where supported.
    fn special_record(path: &Path);
    /// Attempt to replace the held directory by renaming it; expected refusal is host-specific.
    fn replace_directory(path: &Path, moved: &Path) -> Replacement;
    /// Run the storage regression below search-only ancestry and verify its access is unchanged.
    /// Hosts without separate listing/search rights run once with ordinary private ancestry.
    fn with_search_only_ancestor(path: &Path, check: impl FnMut());
    /// A real system program accepted by the shared trust policy, including system links where supported.
    fn system_program() -> PathBuf;
    /// Set up a fresh private record, independent of a user's creation mask.
    fn record(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
        Self::private(path);
    }
}
