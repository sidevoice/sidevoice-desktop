//! Keeping a directory of the app out of the device backup. The app's pairings hold the access token to the person's
//! computer, so they do not travel in an OS backup: after a restore the person pairs again, as with the provider keys
//! (which are `ThisDeviceOnly` on iOS and sealed by a Keystore key that does not migrate on Android).
//!
//! Only iOS asks the app to do it, so the directory the pairings live in is marked each time the app writes there.
//! Android keeps files out by rules its manifest points to (the Android library of `mobile/plugin`), and a computer has
//! no device backup.
use std::{io, path::Path};

#[cfg_attr(target_os = "android", path = "backup/android.rs")]
#[cfg_attr(target_os = "ios", path = "backup/ios.rs")]
#[cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    path = "backup/host.rs"
)]
mod imp;
use imp::Native;

/// What differs per OS: whether the app has to mark a directory to keep it out of the device backup.
trait Os {
    /// Keeps the existing directory at `path`, and what it holds, out of the device backup, wherever the OS asks the app
    /// to.
    fn exclude_from_backup(path: &Path) -> io::Result<()>;
}

/// Keeps the directory at `path`, which must exist, and everything in it out of the device backup. Calling it again on a
/// marked directory changes nothing, so a caller marks the directory before each write into it: the files written there
/// are never outside the mark, and a mark that was not set the first time (or was reset) is set the next.
pub fn exclude_from_backup(path: &Path) -> io::Result<()> {
    Native::exclude_from_backup(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_existing_directory_is_excluded_again_and_again() {
        let home = tempfile::tempdir().unwrap();
        exclude_from_backup(home.path()).unwrap();
        exclude_from_backup(home.path()).unwrap();
    }
}
