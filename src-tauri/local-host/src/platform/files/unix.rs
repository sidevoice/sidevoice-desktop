//! Unix: the mode bits, and the device and inode.

use super::Os;
use std::fs::Permissions;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::time::SystemTime;

pub struct Native;

impl Os for Native {
    fn make_runnable(path: &Path) -> io::Result<()> {
        std::fs::set_permissions(path, Permissions::from_mode(0o700))
    }

    fn identity(path: &Path) -> Option<((u64, u128), SystemTime)> {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        Some(((metadata.dev(), metadata.ino().into()), metadata.modified().ok()?))
    }
}
