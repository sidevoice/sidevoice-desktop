use std::io;

/// Unix filesystem capacity codes retain the installer's existing exact numeric behavior.
pub struct SystemFilesystemErrors;

impl super::FilesystemErrorClassifier for SystemFilesystemErrors {
    fn is_capacity_exhausted(error: &io::Error) -> bool {
        matches!(error.raw_os_error(), Some(libc::ENOSPC) | Some(libc::EDQUOT))
    }

    #[cfg(test)]
    fn capacity_errors_for_test() -> [io::Error; 2] {
        [io::Error::from_raw_os_error(libc::ENOSPC), io::Error::from_raw_os_error(libc::EDQUOT)]
    }
}
