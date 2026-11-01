//! A computer (the crate's tests and checks): `0700` where there is a mode to give, as on a phone. Windows has none; a
//! directory made in a user's profile is that user's.
use super::Os;
use std::{fs::DirBuilder, io, path::Path};

pub struct Native;

impl Os for Native {
    fn create_private_dir(path: &Path) -> io::Result<()> {
        let mut directory = DirBuilder::new();
        directory.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut directory, 0o700);
        directory.create(path)
    }
}
