//! What the install does to files beyond the shared private files of `sidevoice_platform::fs`: letting the installer
//! it unpacked run, and telling whether a record was written since it last looked.
//!
//! What differs per OS is in `files/<os>.rs`: how a file is marked runnable, and which file a path names.

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
    /// is nothing to mark; where who else may run it is only set when the file is created, one open to others is
    /// refused.
    fn make_runnable(_path: &Path) -> io::Result<()> {
        Ok(())
    }

    /// Which file `path` names, never following a link there (Unix: its device and inode; Windows: its volume's serial
    /// number and its file index), and when it was last written, read together; `None` when there is nothing there or
    /// the OS does not say.
    fn identity(_path: &Path) -> Option<((u64, u128), SystemTime)> {
        None
    }
}

/// Lets this user, and nobody else, run the file at `path` (Unix: `0700`; Windows: a file created private stays so,
/// one that is not is refused).
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
    let (identity, modified) = Native::identity(path)?;
    Some(Stamp { identity, modified })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidevoice_desktop_test_support::platform::storage::{Others, StorageFixture, StorageFixtures};
    use sidevoice_platform::fs::{Create, Files, Fs};

    #[test]
    fn a_stamp_changes_when_another_file_takes_the_name_and_not_otherwise() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("install.json");
        assert_eq!(stamp(&path), None);
        std::fs::write(&path, b"{}").unwrap();
        let before = stamp(&path).expect("which file a path names is known on every host");
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
        // As the install unpacks its installer: new and private.
        Files::open_private(&path, Create::NewProgram).unwrap();
        make_runnable(&path).unwrap();
        assert_eq!(StorageFixtures::others(&path), Others::Nothing);
        // One somebody else may read is never made runnable as it is: made this user's alone (Unix) or refused
        // (where who may run it is only set when it is created).
        StorageFixtures::readable(&path);
        match make_runnable(&path) {
            Ok(()) => assert_eq!(StorageFixtures::others(&path), Others::Nothing),
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::PermissionDenied, "{e}"),
        }
    }
}
