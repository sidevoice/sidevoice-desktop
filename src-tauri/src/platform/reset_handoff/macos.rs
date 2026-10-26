//! Preserve the existing per-user temporary control directory, outside reset deletion roots.
use super::ResetHandoff;
use std::{io, path::PathBuf};

pub struct DesktopReset;

impl ResetHandoff for DesktopReset {
    fn reset_available() -> bool {
        true
    }

    fn state_dir() -> io::Result<PathBuf> {
        // SAFETY: getuid has no preconditions. Keep the existing real-uid directory naming contract.
        let uid = unsafe { libc::getuid() };
        Ok(std::env::temp_dir().join(format!("dev.sidevoice.desktop-reset-{uid}")))
    }
}
