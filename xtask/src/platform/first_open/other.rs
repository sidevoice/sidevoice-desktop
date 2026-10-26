//! Hosts other than Windows: no first-open smoke test.

use std::path::{Path, PathBuf};

use super::FirstOpen;
use crate::Result;

pub struct Host;

impl FirstOpen for Host {
    fn binary(repo: &Path) -> PathBuf {
        repo.join("src-tauri/target/release/sidevoice-desktop")
    }

    fn require_fresh_profile() -> Result<()> {
        Err("the first-open smoke test only runs on Windows".into())
    }
}
