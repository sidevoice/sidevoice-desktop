//! One-off carry-over of an install's data from the bundle identifier the app had before `ai.sidevoice.desktop`
//! (`dev.sidevoice.desktop`), so a person who updates keeps their settings, pairing, browser storage and provider keys.
//!
//! REMOVE THIS MODULE two desktop releases after the first one that ships it (RELEASING.md, "Legacy identifier
//! carry-over"), with what exists only for it: `platform::app_data::moves_from` and its trait method `extra_folders`,
//! `platform::webview_data::locations_of`, and `platform::credentials::carry_over`.
//!
//! [`carry_over`] runs at every launch, first, under the launch lock (`reset_handoff::Handoff::await_idle`: no other
//! launch, helper or reset runs meanwhile), before the log, the app data, the webview store or a credential is
//! touched. Every step is idempotent and a no-op once done:
//! - **Folders**: each folder the app keeps for the old identifier (`app_data::moves_from`: the reset's own folder
//!   list, so the app data with the local-host pairing in it, the browser storage, the cache and, on macOS, the logs
//!   and saved window state) is renamed to the new identifier's, only when it exists and the new one does not. Both
//!   are siblings, so it is a rename on one filesystem.
//! - **Provider keys**: copied to the new namespace unless it holds one, then deleted from the old one
//!   (`credentials::carry_over`).
//!
//! It never fails the launch and never deletes old data it did not first have somewhere else: a step that fails is
//! logged and the old data stays, where the next launch tries again. Not carried: macOS's `defaults` domain (system UI
//! state, nothing of the app's own), the macOS microphone grant (the system keys it by bundle id) and the reset's
//! control directory.

use crate::platform::{account, app_data, credentials};
use log::Level;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// The bundle identifier of the builds before `ai.sidevoice.desktop`.
const LEGACY: &str = "dev.sidevoice.desktop";

/// What happened to one old folder that exists.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Renamed to where the app keeps it now.
    Moved,
    /// Left in place: the app already has a folder of its own there, and two are never merged.
    Kept,
    /// Left in place: it could not be moved.
    Failed(io::ErrorKind),
}

/// The log lines of a carry-over. The log does not exist yet when it runs (the migration must come first, as the log
/// plugin creates the log folder), so they wait for [`Report::log`].
pub struct Report(Vec<(Level, String)>);

impl Report {
    /// Write the lines to the log, one per thing carried (or not).
    pub fn log(self) {
        for (level, line) in self.0 {
            log::log!(level, "legacy identifier: {line}");
        }
    }
}

/// Carry the old identifier's folders and provider keys over to the app's own. See the module.
pub fn carry_over() -> Report {
    let mut report = Report(Vec::new());
    match account::home().and_then(|home| app_data::moves_from(&home, LEGACY)) {
        Ok(moves) => folders(&moves, &mut report),
        Err(key) => report.0.push((Level::Info, format!("folders not looked at ({key})"))),
    }
    match credentials::carry_over(LEGACY) {
        Ok(keys) => {
            for (provider, result) in keys {
                report.0.push(match result {
                    Ok(true) => (Level::Info, format!("moved the {provider} key to the new namespace")),
                    Ok(false) => {
                        (Level::Info, format!("removed the old {provider} key: the new namespace already holds one"))
                    }
                    Err(error) => (Level::Warn, format!("could not move the {provider} key: {error:?}")),
                });
            }
        }
        Err(error) => report.0.push((Level::Warn, format!("provider keys not looked at ({error:?})"))),
    }
    report
}

/// Carry each `(old, new)` folder, reporting every one that exists. A failure does not stop the next.
fn folders(moves: &[(PathBuf, PathBuf)], report: &mut Report) {
    for (from, to) in moves {
        let line = match carry(from, to) {
            None => continue,
            Some(Outcome::Moved) => (Level::Info, format!("moved {} to {}", from.display(), to.display())),
            Some(Outcome::Kept) => (Level::Info, format!("kept {}: {} already exists", from.display(), to.display())),
            Some(Outcome::Failed(kind)) => {
                (Level::Warn, format!("could not move {} to {}: {kind}", from.display(), to.display()))
            }
        };
        report.0.push(line);
    }
}

/// The move rule: `from` is renamed to `to` only when `from` exists and `to` does not (a dangling link counts as
/// existing). `None` when there is nothing to carry. Nothing is merged, replaced or deleted, so a failure leaves
/// `from` as it was.
fn carry(from: &Path, to: &Path) -> Option<Outcome> {
    fn exists(path: &Path) -> io::Result<bool> {
        match fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
    match (exists(from), exists(to)) {
        (Ok(false), _) => None,
        (Err(error), _) | (Ok(true), Err(error)) => Some(Outcome::Failed(error.kind())),
        (Ok(true), Ok(true)) => Some(Outcome::Kept),
        (Ok(true), Ok(false)) => Some(match fs::rename(from, to) {
            Ok(()) => Outcome::Moved,
            Err(error) => Outcome::Failed(error.kind()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(path: &Path, text: &str) {
        fs::create_dir_all(path).unwrap();
        fs::write(path.join("settings"), text).unwrap();
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path.join("settings")).unwrap()
    }

    #[test]
    fn an_old_folder_is_renamed_when_the_new_one_is_absent() {
        let temp = tempfile::tempdir().unwrap();
        let (from, to) = (temp.path().join("old"), temp.path().join("new"));
        put(&from, "old data");
        assert_eq!(carry(&from, &to), Some(Outcome::Moved));
        assert!(!from.exists());
        assert_eq!(read(&to), "old data");
        // The next launch: nothing is left to carry.
        assert_eq!(carry(&from, &to), None);
        assert_eq!(read(&to), "old data");
    }

    #[test]
    fn both_present_leaves_both_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let (from, to) = (temp.path().join("old"), temp.path().join("new"));
        put(&from, "old data");
        put(&to, "new data");
        assert_eq!(carry(&from, &to), Some(Outcome::Kept));
        assert_eq!((read(&from).as_str(), read(&to).as_str()), ("old data", "new data"));
        // An empty new folder is still the app's own: Unix `rename` would silently replace it.
        fs::remove_file(to.join("settings")).unwrap();
        assert_eq!(carry(&from, &to), Some(Outcome::Kept));
        assert_eq!(read(&from), "old data");
        assert!(to.is_dir());
    }

    #[test]
    fn neither_present_is_nothing_to_carry() {
        let temp = tempfile::tempdir().unwrap();
        let (from, to) = (temp.path().join("old"), temp.path().join("new"));
        assert_eq!(carry(&from, &to), None);
        assert!(!from.exists() && !to.exists());
        // A new folder alone is the normal state of a fresh install.
        put(&to, "new data");
        assert_eq!(carry(&from, &to), None);
        assert_eq!(read(&to), "new data");
    }

    #[test]
    fn a_failed_move_is_reported_and_leaves_the_old_folder_in_place() {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("old");
        // No parent to rename into.
        let to = temp.path().join("missing").join("new");
        put(&from, "old data");
        assert!(matches!(carry(&from, &to), Some(Outcome::Failed(_))));
        assert_eq!(read(&from), "old data");
        assert!(!to.exists());
    }

    #[test]
    fn a_failure_does_not_stop_the_next_folder_and_every_folder_gets_one_line() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // `failing` cannot be moved: its destination is below a file.
        fs::write(root.join("blocker"), "a file").unwrap();
        let pair = |from: &str, to: PathBuf| (root.join(from), to);
        let moves = vec![
            pair("old-failing", root.join("blocker/new-failing")),
            pair("old-moving", root.join("new-moving")),
            pair("old-kept", root.join("new-kept")),
            pair("old-absent", root.join("new-absent")),
        ];
        put(&moves[0].0, "failing");
        put(&moves[1].0, "moving");
        put(&moves[2].0, "old kept");
        put(&moves[2].1, "new kept");

        let mut report = Report(Vec::new());
        folders(&moves, &mut report);

        assert_eq!(read(&moves[0].0), "failing");
        assert_eq!(read(&moves[1].1), "moving");
        assert!(!moves[1].0.exists());
        assert_eq!((read(&moves[2].0).as_str(), read(&moves[2].1).as_str()), ("old kept", "new kept"));
        let levels: Vec<Level> = report.0.iter().map(|(level, _)| *level).collect();
        assert_eq!(levels, vec![Level::Warn, Level::Info, Level::Info], "one line each, none for the absent");
        assert!(report.0[1].1.starts_with("moved "));
        assert!(report.0[2].1.starts_with("kept "));
    }
}
