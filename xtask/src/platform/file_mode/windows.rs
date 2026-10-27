//! Windows: no Unix permissions.

use std::path::Path;

use super::Os;
use crate::Result;

pub struct Native;

impl Os for Native {
    fn mode(path: &Path) -> Result<u32> {
        Err(format!("{}: no Unix permissions here", path.display()))
    }

    fn chmod(path: &Path, _: u32) -> Result<()> {
        Err(format!("{}: no Unix permissions here", path.display()))
    }
}
