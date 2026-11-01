//! Creating a directory for the app's own files that only the app can open. The sandbox already keeps other apps out of
//! the app's data; this is the directory's own mode, so a record in it is not readable by any other user of the file
//! system either (Unix: `0700`, whatever the process's umask; a computer's per-user profile directory is private already
//! on Windows, and the crate's tests are the only code that runs there).
use std::{io, path::Path};

#[cfg_attr(target_os = "android", path = "private_dir/android.rs")]
#[cfg_attr(target_os = "ios", path = "private_dir/ios.rs")]
#[cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    path = "private_dir/host.rs"
)]
mod imp;
use imp::Native;

/// What differs per OS: how a directory is made private.
trait Os {
    /// Creates the directory at `path`, and any of its parents that is missing, closed to everyone but the app. A
    /// directory that exists is left as it is; a file at `path` is an error.
    fn create_private_dir(path: &Path) -> io::Result<()>;
}

/// Creates the directory at `path` (and its missing parents) private to the app, if it does not exist.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    Native::create_private_dir(path)
}

/// Whether only the owner can open the directory at `path`: its mode, where the OS has one (the tests of this crate ask).
#[cfg(test)]
pub fn only_the_owner_can_open(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777 == 0o700
    }
    #[cfg(not(unix))]
    {
        path.is_dir()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_directory_is_created_with_its_parents_and_an_existing_one_is_left_alone() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("a").join("b");
        create_private_dir(&path).unwrap();
        assert!(path.is_dir());
        std::fs::write(path.join("kept"), b"x").unwrap();
        create_private_dir(&path).unwrap();
        assert_eq!(std::fs::read(path.join("kept")).unwrap(), b"x");
    }

    #[test]
    fn a_file_at_the_path_is_an_error() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("in the way");
        std::fs::write(&path, b"x").unwrap();
        assert!(create_private_dir(&path).is_err());
        assert!(create_private_dir(&path.join("below")).is_err());
    }

    /// Only the owner can open a directory made here, whatever the umask was.
    #[test]
    fn a_new_directory_is_closed_to_everyone_but_the_owner() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("private");
        create_private_dir(&path).unwrap();
        assert!(only_the_owner_can_open(&path));
    }
}
