//! Windows: the `.exe` of `tauri build`; the profile is the Known Folders `RoamingAppData` and `LocalAppData`.

use std::path::{Path, PathBuf};

use super::FirstOpen;
use crate::Result;

/// The app's identifier: the folder Tauri creates under each Known Folder.
const IDENTIFIER: &str = "dev.sidevoice.desktop";

pub struct Host;

impl FirstOpen for Host {
    fn binary(repo: &Path) -> PathBuf {
        repo.join("src-tauri/target/release/sidevoice-desktop.exe")
    }

    fn require_fresh_profile() -> Result<()> {
        // `dirs` resolves both through `SHGetKnownFolderPath`.
        for (name, root) in [("RoamingAppData", dirs::config_dir()), ("LocalAppData", dirs::data_local_dir())] {
            let root = root.ok_or_else(|| format!("Cannot resolve {name}"))?;
            let profile = root.join(IDENTIFIER);
            if profile.exists() {
                return Err(format!(
                    "windows first-open requires a fresh runner: {} already exists",
                    profile.display()
                ));
            }
        }
        Ok(())
    }
}
