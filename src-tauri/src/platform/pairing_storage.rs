//! Host-specific ancestor replacement fixtures for the pairing store's held-directory tests.

use std::path::Path;

#[cfg(unix)]
#[path = "pairing_storage/unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "pairing_storage/windows.rs"]
mod imp;
pub use imp::Os as NativePairingRace;

pub enum Replacement<'a> {
    Redirect(&'a Path),
    Directory,
}

pub trait PairingRace {
    /// Try to rename a held ancestor and put a decoy at its old path. Returns whether replacement
    /// happened: Unix re-verifies and refuses the write; Windows keeps the ancestor held and denies
    /// the rename. The caller checks the saved and decoy records in either case.
    fn replace_ancestor(ancestor: &Path, moved: &Path, replacement: Replacement<'_>) -> bool;
}
