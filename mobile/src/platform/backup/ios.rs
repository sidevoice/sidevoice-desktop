//! iOS: `NSURLIsExcludedFromBackupKey` on the file, which keeps it out of iCloud and computer backups (an encrypted one
//! included). It is an attribute of the file, so a file renamed over it is not marked until this is called on it.
use super::Os;
use objc2_foundation::{NSNumber, NSURLIsExcludedFromBackupKey, NSURL};
use std::{io, path::Path};

pub struct Native;

impl Os for Native {
    fn exclude_from_backup(path: &Path) -> io::Result<()> {
        let url = NSURL::from_file_path(path).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "not an absolute file path")
        })?;
        // SAFETY: the key is Foundation's own constant, and its value is the boolean NSNumber the key takes.
        unsafe {
            url.setResourceValue_forKey_error(
                Some(&NSNumber::new_bool(true)),
                NSURLIsExcludedFromBackupKey,
            )
        }
        .map_err(|_| io::Error::other("the file could not be excluded from the backup"))
    }
}
