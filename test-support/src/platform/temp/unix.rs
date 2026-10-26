use super::TempArea;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub struct Os;

impl TempArea for Os {
    fn short_root() -> PathBuf {
        PathBuf::from("/tmp")
    }

    fn share_sticky(dir: &Path) -> bool {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o1777)).unwrap();
        true
    }

    fn let_group_read(dir: &Path) -> bool {
        let mode = std::fs::metadata(dir).unwrap().permissions().mode();
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode | 0o050)).unwrap();
        true
    }
}
