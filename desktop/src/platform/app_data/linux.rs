//! Linux keeps nothing for the app beyond its folders (the trait's default); a new folder is private from its creation.

use super::Os;
use sidevoice_platform::fs::{Files, Fs};
use std::{io, path::Path};

pub(super) struct Native;

impl Os for Native {
    /// Private (0700) from its creation.
    fn create_folder(path: &Path) -> io::Result<()> {
        Files::create_private_dir(path)
    }
}
