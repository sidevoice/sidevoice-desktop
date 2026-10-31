//! macOS keeps more for the app than its folders: its logs in `~/Library/Logs`, the window state it saves, and the
//! app's `defaults` domain.

use super::{Error, Home, Os, Steps, IDENTIFIER};
use sidevoice_platform::fs::{Files, Fs};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) struct Native;

impl Os for Native {
    /// The logs, then the saved window state, then the `defaults` domain (only for a held home).
    fn delete_extra_data(home: &Home, steps: Steps) -> Result<(), Error> {
        for (name, path) in places(&home.path) {
            if !steps(name, home.remove_entry(&path)) {
                return Err(Error::Stopped);
            }
        }
        // Not a folder: only for a home that was checked and held, so a refused home deletes nothing.
        if !steps("preferences", home.acquired().and_then(|_| remove_preferences())) {
            return Err(Error::Stopped);
        }
        Ok(())
    }

    /// Private (0700) from its creation.
    fn create_folder(path: &Path) -> io::Result<()> {
        Files::create_private_dir(path)
    }
}

/// The extra folders inside `home`, in deletion order, under the report's names.
fn places(home: &Path) -> [(&'static str, PathBuf); 2] {
    [
        ("logs", home.join("Library/Logs").join(IDENTIFIER)),
        ("saved-state", home.join("Library/Saved Application State").join(format!("{IDENTIFIER}.savedState"))),
    ]
}

/// The app's `defaults` domain: deleted when present, and nothing to do when it is not.
fn remove_preferences() -> Result<(), &'static str> {
    let output = Command::new("/usr/bin/defaults").arg("domains").output().map_err(|_| "inspect")?;
    if !output.status.success() {
        return Err("inspect");
    }
    let present = String::from_utf8_lossy(&output.stdout).split(',').any(|d| d.trim() == IDENTIFIER);
    if !present || Command::new("/usr/bin/defaults").args(["delete", IDENTIFIER]).status().is_ok_and(|s| s.success()) {
        Ok(())
    } else {
        Err("remove")
    }
}

/// The macOS deletion set and order, which only macOS has: the common folders, then these extras.
#[cfg(test)]
mod tests {
    use super::super::tests::{macos, named};
    use super::super::Folders;
    use super::*;

    #[test]
    fn the_macos_deletion_is_exactly_the_previous_list() {
        let home = Path::new("/Users/ana");
        let mut all = named(&Folders::from(macos(home)));
        all.extend(places(home));
        let support = home.join("Library/Application Support");
        assert_eq!(
            all,
            vec![
                ("app-data", support.join("ai.sidevoice.desktop")),
                ("models", support.join("Sidevoice/Models")),
                ("webview", home.join("Library/WebKit/ai.sidevoice.desktop")),
                ("cache", home.join("Library/Caches/ai.sidevoice.desktop")),
                ("logs", home.join("Library/Logs/ai.sidevoice.desktop")),
                ("saved-state", home.join("Library/Saved Application State/ai.sidevoice.desktop.savedState")),
            ]
        );
    }
}
