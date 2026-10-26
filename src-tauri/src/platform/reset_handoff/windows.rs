//! Control state is a sibling of the standard data root, independent of reset deletion roots.
use super::ResetHandoff;
use std::{io, path::PathBuf};

pub struct DesktopReset;

impl ResetHandoff for DesktopReset {
    fn reset_available() -> bool {
        false
    }

    fn state_dir() -> io::Result<PathBuf> {
        sidevoice_platform::data_dir::standard()
            .and_then(|path| path.parent().map(|parent| parent.join("dev.sidevoice.desktop-reset")))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "reset state directory unavailable"))
    }
}
