use std::io::{self, ErrorKind};

/// Windows reports disk and quota exhaustion through standard I/O error kinds.
pub struct SystemFilesystemErrors;

impl super::FilesystemErrorClassifier for SystemFilesystemErrors {
    fn is_capacity_exhausted(error: &io::Error) -> bool {
        matches!(error.kind(), ErrorKind::StorageFull | ErrorKind::QuotaExceeded)
    }

    #[cfg(test)]
    fn capacity_errors_for_test() -> [io::Error; 2] {
        [io::Error::from(ErrorKind::StorageFull), io::Error::from(ErrorKind::QuotaExceeded)]
    }
}
