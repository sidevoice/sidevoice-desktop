//! Android: `0700` on the directories it creates, the mode of the app's own data directory.
use super::Os;
use std::{fs::DirBuilder, io, os::unix::fs::DirBuilderExt, path::Path};

pub struct Native;

impl Os for Native {
    fn create_private_dir(path: &Path) -> io::Result<()> {
        DirBuilder::new().recursive(true).mode(0o700).create(path)
    }
}
