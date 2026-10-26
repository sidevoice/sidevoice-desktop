//! Unix: the mode bits, and the device and inode.

use super::Os;
use std::fs::{Metadata, Permissions};
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

pub struct Native;

impl Os for Native {
    fn make_runnable(path: &Path) -> io::Result<()> {
        std::fs::set_permissions(path, Permissions::from_mode(0o700))
    }

    fn identity(metadata: &Metadata) -> Option<(u64, u128)> {
        Some((metadata.dev(), metadata.ino().into()))
    }
}
