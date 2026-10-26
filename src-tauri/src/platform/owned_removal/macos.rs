use super::{Mount, OwnedRemoval, Scope};
use std::{fs::File, path::Path};

pub struct NativeOwnedRemoval;

impl OwnedRemoval for NativeOwnedRemoval {
    fn scope(home: &Path) -> Option<Scope> {
        Some(Scope {
            steps: vec![
                ("app-data", home.join("Library/Application Support/dev.sidevoice.desktop")),
                (
                    "models",
                    home.join("Library/Application Support/Sidevoice").join(sidevoice_platform::data_dir::MODELS),
                ),
                ("webview", home.join("Library/WebKit/dev.sidevoice.desktop")),
                ("cache", home.join("Library/Caches/dev.sidevoice.desktop")),
                ("saved-state", home.join("Library/Saved Application State/dev.sidevoice.desktop.savedState")),
                ("logs", home.join("Library/Logs/dev.sidevoice.desktop")),
            ],
        })
    }

    fn mount(file: &File, _: &Path) -> Result<Mount, &'static str> {
        let stat = rustix::fs::fstat(file).map_err(|_| "inspect-mount")?;
        let fs = rustix::fs::fstatfs(file).map_err(|_| "inspect-mount")?;
        let mut identity = stat.st_dev.to_ne_bytes().to_vec();
        identity.extend(fs.f_mntonname.iter().take_while(|byte| **byte != 0).map(|byte| *byte as u8));
        Ok(Mount(identity))
    }
}
