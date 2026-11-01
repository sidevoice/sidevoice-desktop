//! iOS: `NSURLIsExcludedFromBackupKey` on the directory, which keeps it and everything in it out of iCloud and computer
//! backups (an encrypted one included): Apple's guidance is to move a group of related files into a directory and mark the
//! directory ("Optimizing your app's data for iCloud backup"). Setting it again on a marked directory changes nothing.
use super::Os;
use objc2_foundation::{NSNumber, NSURLIsExcludedFromBackupKey, NSURL};
use std::{io, path::Path};

pub struct Native;

impl Os for Native {
    fn exclude_from_backup(path: &Path) -> io::Result<()> {
        let url = NSURL::from_directory_path(path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "not an absolute directory path",
            )
        })?;
        // SAFETY: the key is Foundation's own constant, and its value is the boolean NSNumber the key takes.
        unsafe {
            url.setResourceValue_forKey_error(
                Some(&NSNumber::new_bool(true)),
                NSURLIsExcludedFromBackupKey,
            )
        }
        .map_err(|_| io::Error::other("the directory could not be excluded from the backup"))
    }
}
