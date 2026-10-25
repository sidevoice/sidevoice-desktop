//! Classify local filesystem capacity failures for installer reporting.
use std::io;

/// Recognize disk exhaustion and per-user quota exhaustion without exposing native error codes.
pub trait FilesystemErrorClassifier {
    /// True only for a full filesystem or exhausted filesystem quota.
    fn is_capacity_exhausted(error: &io::Error) -> bool;

    /// Representative native capacity errors, for portable interface and installer-key tests.
    #[cfg(test)]
    fn capacity_errors_for_test() -> [io::Error; 2];
}

#[cfg(unix)]
#[path = "filesystem_error/unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "filesystem_error/windows.rs"]
mod imp;

pub use imp::SystemFilesystemErrors;

#[cfg(test)]
mod tests {
    use super::{FilesystemErrorClassifier, SystemFilesystemErrors};
    use std::io::{self, ErrorKind};

    #[test]
    fn capacity_errors_and_ordinary_errors_use_the_interface() {
        for error in SystemFilesystemErrors::capacity_errors_for_test() {
            assert!(SystemFilesystemErrors::is_capacity_exhausted(&error));
        }
        for kind in [ErrorKind::PermissionDenied, ErrorKind::NotFound, ErrorKind::Interrupted] {
            assert!(!SystemFilesystemErrors::is_capacity_exhausted(&io::Error::from(kind)));
        }
    }

    #[cfg(unix)]
    #[test]
    fn constructed_capacity_kinds_keep_the_original_raw_code_rule() {
        for kind in [ErrorKind::StorageFull, ErrorKind::QuotaExceeded] {
            assert!(!SystemFilesystemErrors::is_capacity_exhausted(&io::Error::from(kind)));
        }
    }
}
