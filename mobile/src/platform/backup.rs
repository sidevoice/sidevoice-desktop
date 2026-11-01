//! Keeping a file of the app out of the device backup. The app's pairings hold the access token to the person's
//! computer, so they do not travel in an OS backup: after a restore the person pairs again, as with the provider keys
//! (which are `ThisDeviceOnly` on iOS and sealed by a Keystore key that does not migrate on Android).
//!
//! Only iOS asks the app to do it, file by file, so the file is marked each time it is written. Android keeps files out
//! by rules its manifest points to (the Android library of `mobile/plugin`), and a computer has no device backup.
use std::{io, path::Path};

#[cfg_attr(target_os = "android", path = "backup/android.rs")]
#[cfg_attr(target_os = "ios", path = "backup/ios.rs")]
#[cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    path = "backup/host.rs"
)]
mod imp;
use imp::Native;

/// What differs per OS: whether the app has to mark a file to keep it out of the device backup.
trait Os {
    /// Keeps the existing file at `path` out of the device backup, wherever the OS asks the app to.
    fn exclude_from_backup(path: &Path) -> io::Result<()>;
}

/// Keeps the file at `path`, which must exist, out of the device backup. The mark belongs to the file, not to its name:
/// a file that replaces it (an atomic write renames a new one over the old) is unmarked until this is called on it.
pub fn exclude_from_backup(path: &Path) -> io::Result<()> {
    Native::exclude_from_backup(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_existing_file_is_excluded() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("record");
        std::fs::write(&path, b"{}").unwrap();
        exclude_from_backup(&path).unwrap();
    }
}
