//! Where tests make their scratch directories, and the directory modes the portable test-file operations lack.
use std::path::{Path, PathBuf};

#[cfg(unix)]
#[path = "temp/unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "temp/windows.rs"]
mod imp;

use imp::Os;

/// The only parts of the scratch area that differ per OS.
trait Area {
    /// A directory whose children get paths short enough for a local socket, and which a private child is safe in.
    fn short_root() -> PathBuf {
        std::env::temp_dir()
    }

    /// Makes `dir` writable by everybody, with deletion limited to each file's owner. `false`, changing nothing, where
    /// the OS has no such mode.
    fn share_sticky(_dir: &Path) -> bool {
        false
    }

    /// Lets the owner's group (not everybody) read and enter `dir`. `false`, changing nothing, where the OS has no
    /// group-only access.
    fn let_group_read(_dir: &Path) -> bool {
        false
    }

    /// The root of the system's volume, when an ordinary user may make folders directly in it and they inherit from it
    /// access for other users. `None` where there is no such place (Unix: `/` is root's).
    fn shared_volume_root() -> Option<PathBuf> {
        None
    }
}

/// A directory whose children get paths short enough for a local socket, and which a private child is safe in.
pub fn short_root() -> PathBuf {
    Os::short_root()
}

/// Makes `dir` writable by everybody, with deletion limited to each file's owner (the sticky bit, as `/tmp` is).
/// Returns `false`, changing nothing, where the OS has no such mode.
pub fn share_sticky(dir: &Path) -> bool {
    Os::share_sticky(dir)
}

/// Lets the owner's group (not everybody) read and enter `dir`. Returns `false`, changing nothing, where the OS has no
/// group-only access.
pub fn let_group_read(dir: &Path) -> bool {
    Os::let_group_read(dir)
}

/// A fresh folder directly below the system volume's root, with the access new folders there inherit, removed (with
/// what is in it) when dropped.
pub struct BelowVolumeRoot(PathBuf);

impl BelowVolumeRoot {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for BelowVolumeRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fresh folder directly below the system volume's root, as a temporary directory moved there is (`C:\Temp`): on
/// Windows it inherits the root's "Authenticated Users: Modify", so other users can replace what is in it. `None`
/// where an ordinary user cannot make one there (Unix).
pub fn below_volume_root() -> Option<BelowVolumeRoot> {
    let root = Os::shared_volume_root()?;
    let mut random = [0u8; 6];
    getrandom::fill(&mut random).unwrap();
    let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let dir = root.join(format!("sidevoice-test-{name}"));
    std::fs::create_dir(&dir).unwrap();
    Some(BelowVolumeRoot(dir))
}
