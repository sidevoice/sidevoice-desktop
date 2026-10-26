//! Linux: systemd's user manager. A job is the user unit `<name>.service` (sidevoice-connector
//! `platform/service/linux.rs`), written to `systemd/user` in this user's configuration directory and asked of
//! `systemctl --user`. The running programs come from the process list (the trait's default). Without a user manager
//! that answers, nothing is proven absent.

use super::{Os, Seen};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) struct Native;

impl Os for Native {
    fn job(home: &Path, _label: &str, name: &str) -> Seen {
        let unit = format!("{}.service", name);
        // The unit's file where the connector writes it: `$XDG_CONFIG_HOME` (which the manager's environment may
        // set differently from this process's) or `~/.config`. Any file there is a job, loaded or not.
        let configs: Vec<PathBuf> = dirs::config_dir().into_iter().chain([home.join(".config")]).collect();
        for config in configs {
            match std::fs::symlink_metadata(config.join("systemd/user").join(&unit)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Ok(_) => return Seen::Present,
                Err(_) => return Seen::Unknown,
            }
        }
        // The manager's own answer, wherever it reads units from: absent only when it says `not-found`.
        let Ok(output) =
            Command::new("systemctl").args(["--user", "show", "--property=LoadState", "--", &unit]).output()
        else {
            return Seen::Unknown;
        };
        if !output.status.success() {
            return Seen::Unknown;
        }
        let shown = String::from_utf8_lossy(&output.stdout);
        match shown.lines().find_map(|line| line.trim().strip_prefix("LoadState=")) {
            Some("not-found") => Seen::Absent,
            Some(state) if !state.is_empty() => Seen::Present,
            _ => Seen::Unknown,
        }
    }
}
