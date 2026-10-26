//! Host-specific ancestor replacement fixtures for the pairing store's held-directory tests.

use std::path::Path;

#[cfg_attr(unix, path = "pairing_storage/unix.rs")]
#[cfg_attr(windows, path = "pairing_storage/windows.rs")]
mod imp;
use imp::Native;

pub enum Replacement<'a> {
    Redirect(&'a Path),
    Directory,
}

/// What differs per OS: whether a held ancestor can be replaced at all.
trait Os {
    /// Attempt the replacement; whether it happened.
    fn replace_ancestor(ancestor: &Path, moved: &Path, replacement: Replacement<'_>) -> bool;
}

/// Try to rename a held ancestor and put a decoy at its old path. Returns whether replacement
/// happened: Unix re-verifies and refuses the write; Windows keeps the ancestor held and denies
/// the rename. The caller checks the saved and decoy records in either case.
pub fn replace_ancestor(ancestor: &Path, moved: &Path, replacement: Replacement<'_>) -> bool {
    Native::replace_ancestor(ancestor, moved, replacement)
}
