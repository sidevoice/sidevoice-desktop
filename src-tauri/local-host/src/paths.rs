//! Where the connector and the core keep their files (SEAMS §1): `D` is the one data directory every Sidevoice program
//! shares (`sidevoice-platform`'s `data_dir`: `$SIDEVOICE_DATA_DIR`, else the OS's standard place), `C` = `D/core` the
//! core's. The app writes in neither: it reads `D/install.json`, the endpoint records the programs publish
//! (`D/connector.json`, `C/core.json`, see [`crate::endpoints`]) and shows the logs.

use sidevoice_platform::data_dir;
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

    /// `D` by the shared rule: `$SIDEVOICE_DATA_DIR` when set (absolute), else this OS's standard place. `None` when
    /// the user has none (or the variable is not absolute): then there is no local host to look for.
    pub fn from_env() -> Option<Self> {
        data_dir::resolve(|name| std::env::var_os(name)).ok().map(|data| DataDirs { data })
    }

    /// `C`, the core's directory: 0700 and this user's, or the app does not connect.
    pub fn core(&self) -> PathBuf {
        data_dir::core_dir(&self.data)
    }

    /// `C/core.json`: the core's record, where it publishes `endpoint`.
    pub fn core_record(&self) -> PathBuf {
        self.core().join("core.json")
    }

    /// `D/connector.json`: the connector's record, `{pid, endpoint}`.
    pub fn connector_record(&self) -> PathBuf {
        self.data.join("connector.json")
    }

    /// `D/install.json`: the active installation and the `command` that runs the CLI.
    pub fn install_record(&self) -> PathBuf {
        self.data.join("install.json")
    }

    /// `R`, the connector's releases (`D/install`).
    pub fn releases(&self) -> PathBuf {
        data_dir::releases_dir(&self.data)
    }

    /// The log "Ver registro" shows: the core's, else the connector's.
    pub fn log(&self) -> Option<PathBuf> {
        ["core.log", "connector.log"].iter().map(|name| self.data.join(name)).find(|p| is_file(p))
    }
}

/// What "Ver registro" shows: the app's own log first, then the core's (else the connector's) one, each only if it is
/// a file. The app's is there with or without a connector.
pub fn logs_to_show(app: Option<PathBuf>, connector: Option<PathBuf>) -> Vec<PathBuf> {
    app.into_iter().chain(connector).filter(|p| is_file(p)).collect()
}

fn is_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).map(|m| m.is_file()).unwrap_or(false)
}

#[cfg(test)]
mod log_tests {
    use super::*;

    #[test]
    fn shows_the_app_log_then_the_connectors_and_only_what_exists() {
        let dir = tempfile::tempdir().unwrap();
        let (app, core) = (dir.path().join("app.log"), dir.path().join("core.log"));
        let missing = dir.path().join("nope.log");
        assert!(logs_to_show(None, None).is_empty());
        assert!(logs_to_show(Some(missing.clone()), None).is_empty());
        std::fs::write(&app, "x").unwrap();
        assert_eq!(logs_to_show(Some(app.clone()), None), vec![app.clone()]);
        assert_eq!(logs_to_show(Some(app.clone()), Some(missing)), vec![app.clone()]);
        std::fs::write(&core, "x").unwrap();
        assert_eq!(logs_to_show(Some(app.clone()), Some(core.clone())), vec![app, core]);
    }
}
