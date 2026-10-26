//! Hosts without a creation mask: nothing to hold.

use super::PrivateCreation;

pub struct PrivateFiles;

impl PrivateCreation for PrivateFiles {
    fn start() -> PrivateFiles {
        PrivateFiles
    }
}
