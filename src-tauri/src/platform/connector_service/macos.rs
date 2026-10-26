//! macOS: launchd. A job is the LaunchAgent `~/Library/LaunchAgents/<label>.plist` loaded in this user's GUI domain
//! (`gui/<uid>/<label>`); the running programs come from `ps`.

use super::{Os, Seen};
use std::path::Path;
use std::process::Command;

pub(super) struct Native;

impl Os for Native {
    fn job(home: &Path, label: &str, _name: &str) -> Seen {
        match std::fs::symlink_metadata(home.join("Library/LaunchAgents").join(format!("{}.plist", label))) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => return Seen::Present,
            Err(_) => return Seen::Unknown,
        }
        let target = format!("gui/{}/{}", unsafe { libc::geteuid() }, label);
        let Ok(output) = Command::new("/bin/launchctl").args(["print", &target]).output() else { return Seen::Unknown };
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
        let output = Command::new("/bin/ps").args(["-axo", "comm="]).output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).lines().map(str::to_owned).collect())
    }
}
