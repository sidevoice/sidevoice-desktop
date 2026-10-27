//! Unix (macOS, Linux): the user database entry of the effective user (`getpwuid_r`), never `HOME`; and what this
//! user may change in a directory, from its owner, its sticky bit and `access(2)` (which applies ACLs and groups).

use nix::errno::Errno;
use nix::unistd::{access, AccessFlags, Uid, User};
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub(super) struct Native;

impl super::Os for Native {
    fn recorded_home() -> Option<PathBuf> {
        User::from_uid(Uid::effective()).ok().flatten().map(|user| user.dir)
    }

    /// The owner of `dir` may change its mode and so its entries. Otherwise renaming an entry away and creating
    /// another under its name both need write and search on `dir`; under the sticky bit only the entry's owner (or
    /// `dir`'s) may rename it.
    fn may_replace(dir: &Path, child: &Path) -> io::Result<bool> {
        let me = Uid::effective().as_raw();
        let directory = std::fs::symlink_metadata(dir)?;
        if directory.uid() == me {
            return Ok(true);
        }
        match access(dir, AccessFlags::W_OK | AccessFlags::X_OK) {
            Ok(()) => {}
            Err(Errno::EACCES | Errno::EROFS) => return Ok(false),
            Err(error) => return Err(error.into()),
        }
        Ok(directory.permissions().mode() & 0o1000 == 0 || std::fs::symlink_metadata(child)?.uid() == me)
    }
}
