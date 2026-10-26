use super::Area;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub struct Os;

impl Area for Os {
    fn short_root() -> PathBuf {
        PathBuf::from("/tmp")
    }

    fn share_sticky(dir: &Path) -> bool {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o1777)).unwrap();
        true
    }
}
