//! Windows: the `.exe` of `tauri build` started on a runner; the profile is the Known Folders `RoamingAppData` and
//! `LocalAppData`. Its installer comes from `cargo xtask bundle`.

use std::path::PathBuf;

use super::Os;
use crate::util::repo;
use crate::Result;

/// The app's identifier: the folder Tauri creates under each Known Folder.
const IDENTIFIER: &str = "dev.sidevoice.desktop";

pub struct Native;

impl Os for Native {
    fn app_binary() -> Option<PathBuf> {
        Some(repo().join("src-tauri/target/release/sidevoice-desktop.exe"))
    }

    /// Tauri resolves settings and webview data from the Known Folders, not from environment overrides, and a person's
    /// profile is never deleted.
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
