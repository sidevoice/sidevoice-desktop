//! Where the connector and the core keep their files (SEAMS §1): `D` is the connector's data directory
//! (`$SIDEVOICE_DATA_DIR`, else `~/.sidevoice`), `C` = `D/core` the core's. The app writes in neither: it only reads
//! `D/install.json`, connects to `D/connector.sock` and `C/local.sock`, and shows the logs.

use std::path::{Path, PathBuf};

/// The connector's and the core's directories, named once where the app starts and passed down from there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDirs {
    /// `D`.
    pub data: PathBuf,
}

impl DataDirs {
    pub fn new(data: impl Into<PathBuf>) -> Self {
        DataDirs { data: data.into() }
    }

    /// `D` as the connector resolves it: `$SIDEVOICE_DATA_DIR` when set (absolute), else `$HOME/.sidevoice`. `None`
    /// when neither names an absolute directory: then there is no local host to look for.
    pub fn from_env() -> Option<Self> {
        let named = std::env::var_os("SIDEVOICE_DATA_DIR").filter(|v| !v.is_empty()).map(PathBuf::from);
        let data = match named {
            Some(dir) => dir,
            None => PathBuf::from(std::env::var_os("HOME").filter(|v| !v.is_empty())?).join(".sidevoice"),
        };
        data.is_absolute().then_some(DataDirs { data })
    }

    /// `C`, the core's directory: 0700 and this user's, or the app does not connect.
    pub fn core(&self) -> PathBuf {
        self.data.join("core")
    }

    /// `C/local.sock`: the core's private listener.
    pub fn core_socket(&self) -> PathBuf {
        self.core().join("local.sock")
    }

    /// `D/connector.sock`: the connector's IPC (JSON lines).
    pub fn connector_socket(&self) -> PathBuf {
        self.data.join("connector.sock")
    }

    /// `D/install.json`: the active installation and the `command` that runs the CLI.
    pub fn install_record(&self) -> PathBuf {
        self.data.join("install.json")
    }

    /// The log "Ver registro" shows: the core's, else the connector's.
    pub fn log(&self) -> Option<PathBuf> {
        ["core.log", "connector.log"].iter().map(|name| self.data.join(name)).find(|p| is_file(p))
    }
}

fn is_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).map(|m| m.is_file()).unwrap_or(false)
}
