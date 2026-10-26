//! What the install does to files beyond the shared private files of `sidevoice_platform::fs`: letting the installer
//! it unpacked run, and telling whether a record was written since it last looked.
//!
//! What differs per OS is in `files/<os>.rs`: how a file is marked runnable, and which file a path names.

use std::fs::Metadata;
use std::io;
use std::path::Path;
use std::time::SystemTime;

#[cfg_attr(unix, path = "files/unix.rs")]
#[cfg_attr(windows, path = "files/windows.rs")]
mod imp;
use imp::Native;

/// What differs per OS.
trait Os {
    /// Lets this user, and nobody else, run the file at `path`. Where running is not a permission on the file, there
    /// is nothing to do.
    fn make_runnable(_path: &Path) -> io::Result<()> {
        Ok(())
    }

    /// Which file `metadata` describes (Unix: its device and inode); `None` where the OS does not say.
    fn identity(_metadata: &Metadata) -> Option<(u64, u128)> {
        None
    }
}

/// Lets this user, and nobody else, run the file at `path` (Unix: `0700`).
pub fn make_runnable(path: &Path) -> io::Result<()> {
    Native::make_runnable(path)
}

/// A file as it stands: which file a path names, and when it was last written.
#[derive(Debug, PartialEq, Eq)]
pub struct Stamp {
    identity: (u64, u128),
    modified: SystemTime,
}

/// What `path` names now, never following a link there: another reading differs once the file was written, or
/// another took its name (a rename over it). `None` when there is nothing there or it cannot be told.
pub fn stamp(path: &Path) -> Option<Stamp> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    Some(Stamp { identity: Native::identity(&metadata)?, modified: metadata.modified().ok()? })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidevoice_desktop_test_support::platform::storage::{Others, StorageFixture, StorageFixtures};

    #[test]
    fn a_stamp_changes_when_another_file_takes_the_name_and_not_otherwise() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("install.json");
        assert_eq!(stamp(&path), None);
        std::fs::write(&path, b"{}").unwrap();
        let Some(before) = stamp(&path) else { return }; // A host that cannot tell.
        assert_eq!(stamp(&path), Some(before));
        let other = tmp.path().join("other.json");
        std::fs::write(&other, b"{}").unwrap();
        let before = stamp(&path);
        std::fs::rename(&other, &path).unwrap();
        assert_ne!(stamp(&path), before);
    }

    #[test]
    fn a_runnable_file_stays_this_users() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("program");
        std::fs::write(&path, b"").unwrap();
        make_runnable(&path).unwrap();
        assert_eq!(StorageFixtures::others(&path), Others::Nothing);
    }
}
