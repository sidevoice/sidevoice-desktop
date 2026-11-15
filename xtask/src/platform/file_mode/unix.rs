//! Unix: the file's mode.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::Os;
use crate::Result;

pub struct Native;

impl Os for Native {
    fn mode(path: &Path) -> Result<u32> {
        let meta = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(meta.permissions().mode() & 0o7777)
    }

    fn chmod(path: &Path, mode: u32) -> Result<()> {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| format!("{}: {e}", path.display()))
    }
}
