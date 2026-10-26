//! Unix: the process creation mask.

use super::PrivateCreation;

pub struct PrivateFiles {
    before: rustix::fs::Mode,
}

impl PrivateCreation for PrivateFiles {
    fn start() -> PrivateFiles {
        PrivateFiles { before: rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077)) }
    }
}

impl Drop for PrivateFiles {
    fn drop(&mut self) {
        rustix::process::umask(self.before);
    }
}
