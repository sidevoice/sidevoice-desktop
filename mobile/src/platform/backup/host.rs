//! A computer (the crate's tests and checks): there is no device backup of the app's files.
use super::Os;
use std::{io, path::Path};

pub struct Native;

impl Os for Native {
    fn exclude_from_backup(_path: &Path) -> io::Result<()> {
        Ok(())
    }
}
