use super::TempArea;
use std::path::{Path, PathBuf};

pub struct Os;

impl TempArea for Os {
    fn short_root() -> PathBuf {
        std::env::temp_dir()
    }

    fn share_sticky(_dir: &Path) -> bool {
        false
    }
}
