//! Android: nothing to do on the directory. The backup rules in the Android library of `mobile/plugin` name what
//! stays out, and the system reads them.
use super::Os;
use std::{io, path::Path};

pub struct Native;

impl Os for Native {
    fn exclude_from_backup(_path: &Path) -> io::Result<()> {
        Ok(())
    }
}
