use super::{Mount, OwnedRemoval, Scope};
use std::{fs::File, path::Path};

pub struct NativeOwnedRemoval;

impl OwnedRemoval for NativeOwnedRemoval {
    fn scope(home: &Path) -> Option<Scope> {
        Some(Scope {
            directories: [
                "Library/Application Support/dev.sidevoice.desktop",
                "Library/WebKit/dev.sidevoice.desktop",
                "Library/Caches/dev.sidevoice.desktop",
                "Library/Saved Application State/dev.sidevoice.desktop.savedState",
                "Library/Logs/dev.sidevoice.desktop",
            ]
            .map(|name| home.join(name))
            .into(),
            models: home.join("Library/Application Support/Sidevoice").join(sidevoice_platform::data_dir::MODELS),
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
