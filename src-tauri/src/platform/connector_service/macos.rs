//! macOS: launchd. A job is the LaunchAgent `~/Library/LaunchAgents/<label>.plist` loaded in this user's GUI domain
//! (`gui/<uid>/<label>`); the running programs come from `ps`. Both commands go through the bounded runner.

use super::{run, Os, Seen};
use std::path::Path;

pub(super) struct Native;

impl Os for Native {
    fn job(home: &Path, label: &str, _name: &str) -> Seen {
        match std::fs::symlink_metadata(home.join("Library/LaunchAgents").join(format!("{}.plist", label))) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => return Seen::Present,
            Err(_) => return Seen::Unknown,
        }
        let target = format!("gui/{}/{}", nix::unistd::Uid::effective(), label);
        let Some(output) = run("/bin/launchctl", &["print", &target]) else { return Seen::Unknown };
        if output.status.success() {
            return Seen::Present;
        }
        let detail = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        if output.status.code() == Some(113) || detail.contains("could not find service") {
            Seen::Absent
        } else {
            Seen::Unknown
        }
    }

    fn programs() -> Option<Vec<String>> {
        let output = run("/bin/ps", &["-axo", "comm="])?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).lines().map(str::to_owned).collect())
    }
}
