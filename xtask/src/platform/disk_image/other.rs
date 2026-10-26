//! Hosts without `tiffutil` and `hdiutil`: no disk image.

use std::path::Path;

use super::DiskImage;
use crate::Result;

pub struct Host;

impl DiskImage for Host {
    fn assemble(&self, _: &Path, _: &Path, _: [&Path; 2], _: &Path, _: &Path, _: &Path) -> Result<()> {
        Err("a .dmg can only be built on macOS".into())
    }
}
