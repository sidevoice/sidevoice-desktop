//! One-off carry-over of an install's data from the bundle identifier the app had before `ai.sidevoice.desktop`
//! (`dev.sidevoice.desktop`), so a person who updates keeps their settings, pairing, browser storage and provider keys.
//! It also serves the builds that already ran under the new identifier with no migration (the nightly of #316): their
//! folders are first-start leftovers, and the old install's data is still where it was.
//!
//! REMOVE THIS MODULE two desktop releases after the first one that ships it (RELEASING.md, "Legacy identifier
//! carry-over"), with what exists only for it: `platform::app_data::moves_from` and its trait method `extra_folders`,
//! `platform::webview_data::locations_of`, `platform::credentials::{carry_over, holds_keys}`, and
//! `data::PAIRINGS_FILE` unless something else uses it.
//!
//! [`carry_over`] runs at every launch, first, under the launch lock (`reset_handoff::Handoff::await_idle`: no other
//! launch, helper or reset runs meanwhile), before the log, the app data, the webview store or a credential is
//! touched, until it leaves [`MARKER`] in the new app data folder. With the marker it does nothing but look for it.
//! Without it:
//! - **Folders**: each folder the app keeps for the old identifier (`app_data::moves_from`: the reset's own folder
//!   list, so the app data with the local-host pairing in it, the browser storage, the cache and, on macOS, the logs
//!   and saved window state) that exists is carried to the new identifier's.
//!   - The new one is absent: the old one is renamed to it (siblings: a rename on one filesystem).
//!   - The new one exists and the install was never set up (`configured`: no settings, remote pairings or card
//!     placement file in the new app data folder, and no provider key in the new namespace): it holds only what
//!     a first start makes, and the old folder replaces it. It is set aside, the old one renamed in, and only then the
//!     first-start folder deleted; if the rename fails it is put back. One thing a first start makes outlives it: the
//!     local host's pairing (`local-host.json`), made on its own with a core running here, which revoked the old
//!     install's, so the new file is copied over the old one's.
//!   - The new one exists and the install is set up: the new one stays, the old one is left in place and logged. Two
//!     installs are never merged.
//! - **Provider keys**: each old key without a new one is copied to the new namespace, read back, and deleted from the
//!   old one (`credentials::carry_over`); a key the new namespace holds already wins.
//! - **The marker** is written once every step ended in a definite state (carried, kept, or nothing there), including
//!   an install with nothing to carry. A step that failed leaves it unwritten, and the next launch tries again.
//!
//! It never fails the launch and never deletes old data it did not first have somewhere else: the old data is renamed
//! or copied-then-deleted, and a step that fails is logged and leaves it in place. Not carried: macOS's `defaults`
//! domain (system UI state, nothing of the app's own), the macOS microphone grant (the system keys it by bundle id)
//! and the reset's control directory.

use crate::data::PAIRINGS_FILE;
use crate::platform::app_data::{self, Moves};
use crate::platform::{account, credentials};
use log::Level;
use sidevoice_desktop_core::call_controls::PLACEMENTS_FILE;
use sidevoice_desktop_core::settings;
use sidevoice_local_host::store;
use sidevoice_platform::fs::{Files, Fs};
use std::{fs, io, path::Path};

/// The bundle identifier of the builds before `ai.sidevoice.desktop`.
const LEGACY: &str = "dev.sidevoice.desktop";

/// Left in the new app data folder once the carry-over has run (or found nothing to carry). Inside that folder, so a
/// reset, which deletes it, starts the next launch afresh.
const MARKER: &str = "legacy-identifier.done";

/// What happened to one old folder that exists.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Renamed to where the app keeps it now.
    Moved,
    /// The app's folder there held only first-start files: the old one took its place.
    Replaced,
    /// Left in place: the app already has a set-up folder of its own there, and two are never merged.
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
    match account::home().and_then(|home| app_data::moves_from(&home, LEGACY)) {
        Ok(moves) => run(&moves, || credentials::holds_keys().unwrap_or(false), || credentials::carry_over(LEGACY)),
        Err(key) => Report(vec![(Level::Info, format!("not looked at ({key})"))]),
    }
}

/// [`carry_over`] for these folders. `keys_held` says whether the new namespace holds a provider key (asked once,
/// before anything is carried); `carry_keys` carries the old keys (once, after the folders).
fn run(
    moves: &Moves,
    keys_held: impl FnOnce() -> bool,
    carry_keys: impl FnOnce() -> Result<credentials::Carried, credentials::Error>,
) -> Report {
    let mut report = Report(Vec::new());
    if exists(&moves.data.1.join(MARKER)).unwrap_or(false) {
        return report;
    }
    // Read before anything moves.
    let configured = configured(&moves.data.1) || keys_held();
    let mut done = folders(moves, configured, &mut report);
    match carry_keys() {
        Ok(keys) => {
            for (provider, result) in keys {
                report.0.push(match result {
                    Ok(true) => (Level::Info, format!("moved the {provider} key to the new namespace")),
                    Ok(false) => {
                        (Level::Info, format!("removed the old {provider} key: the new namespace already holds one"))
                    }
                    Err(error) => {
                        done = false;
                        (Level::Warn, format!("could not move the {provider} key: {error:?}"))
                    }
                });
            }
        }
        Err(error) => {
            done = false;
            report.0.push((Level::Warn, format!("provider keys not looked at ({error:?})")));
        }
    }
    if done {
        if let Err(error) = write_marker(&moves.data.1) {
            report.0.push((Level::Warn, format!("could not write {MARKER}: {}", error.kind())));
        }
    }
    report
}

/// Carry each `(old, new)` folder, reporting every one that exists; whether none failed. A failure does not stop the
/// next folder, but the app data goes last and only when the rest went through: `configured` is read from it, so a
/// retry after a failure must find it as this run did.
fn folders(moves: &Moves, configured: bool, report: &mut Report) -> bool {
    let mut ok = true;
    for (from, to) in &moves.others {
        ok &= step(from, to, configured, &[], report);
    }
    ok && step(&moves.data.0, &moves.data.1, configured, &[store::FILE_NAME], report)
}

/// One folder: carried, reported; false when it failed.
fn step(from: &Path, to: &Path, configured: bool, keep: &[&str], report: &mut Report) -> bool {
    let outcome = carry(from, to, configured, keep);
    let (from, to) = (from.display(), to.display());
    let (ok, line) = match outcome {
        None => return true,
        Some(Outcome::Moved) => (true, (Level::Info, format!("moved {from} to {to}"))),
        Some(Outcome::Replaced) => {
            (true, (Level::Info, format!("moved {from} to {to}, replacing what a first start had made there")))
        }
        Some(Outcome::Kept) => (true, (Level::Info, format!("kept {from}: {to} already exists and is set up"))),
        Some(Outcome::Failed(kind)) => (false, (Level::Warn, format!("could not move {from} to {to}: {kind}"))),
    };
    report.0.push(line);
    ok
}

/// The move rule. `None` when `from` does not exist: nothing to carry. Else `from` is renamed to `to` when `to`
/// does not exist (a dangling link counts as existing), and replaces it when `to` exists but the install is not
/// `configured` (see `replace` for `keep`). Nothing is merged or deleted before it is somewhere else, so a failure
/// leaves `from` as it was.
fn carry(from: &Path, to: &Path, configured: bool, keep: &[&str]) -> Option<Outcome> {
    match (exists(from), exists(to)) {
        (Ok(false), _) => None,
        (Err(error), _) | (Ok(true), Err(error)) => Some(Outcome::Failed(error.kind())),
        (Ok(true), Ok(false)) => Some(match fs::rename(from, to) {
            Ok(()) => Outcome::Moved,
            Err(error) => Outcome::Failed(error.kind()),
        }),
        (Ok(true), Ok(true)) if configured => Some(Outcome::Kept),
        (Ok(true), Ok(true)) => Some(replace(from, to, keep, |from, to| fs::rename(from, to))),
    }
}

/// `to` holds only what a first start made: `from` takes its place. `to` is set aside first and put back if `from`
/// cannot be renamed in; it is deleted only once `from` is in place (and `from` is renamed, never copied, so it is
/// never deleted). The files `keep` names are copied from the first-start folder over the old one's, which they
/// supersede. `rename` is `fs::rename`, a parameter so a test can fail it.
fn replace(from: &Path, to: &Path, keep: &[&str], rename: impl Fn(&Path, &Path) -> io::Result<()>) -> Outcome {
    let mut name = to.file_name().unwrap_or_default().to_os_string();
    name.push(".first-start");
    let aside = to.with_file_name(name);
    // A leftover of an earlier run is only ever a first-start folder of ours.
    match fs::remove_dir_all(&aside) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Outcome::Failed(error.kind()),
    }
    if let Err(error) = rename(to, &aside) {
        return Outcome::Failed(error.kind());
    }
    match rename(from, to) {
        Ok(()) => {
            // What a first start made that outlives it, best effort: see `carry_over`.
            for name in keep {
                if exists(&aside.join(name)).unwrap_or(false) {
                    let _ = fs::copy(aside.join(name), to.join(name));
                }
            }
            // Best effort: what is left is a first-start folder, harmless, and deleted by the next replacement.
            let _ = fs::remove_dir_all(&aside);
            Outcome::Replaced
        }
        Err(error) => {
            let _ = rename(&aside, to);
            Outcome::Failed(error.kind())
        }
    }
}

/// Whether the app's data folder `data` holds anything the person did: the files the app writes only on their action
/// (the settings, the card's placements, the remote pairings). A first start writes none of them; the local host's
/// pairing is not one, as the app makes it by itself when a core runs here. A folder that cannot be read counts as
/// set up.
fn configured(data: &Path) -> bool {
    [settings::FILE_NAME, PLACEMENTS_FILE, PAIRINGS_FILE].iter().any(|name| exists(&data.join(name)).unwrap_or(true))
}

fn exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Leave [`MARKER`] in `data`, creating the folder, private, if the app has not yet.
fn write_marker(data: &Path) -> io::Result<()> {
    if !exists(data)? {
        Files::ensure_private_dir(data)?;
    }
    fs::write(data.join(MARKER), format!("carried over from {LEGACY}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use credentials::Error;
    use std::path::PathBuf;

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
        assert_eq!(carry(&from, &to, false, &[]), Some(Outcome::Moved));
        assert!(!from.exists());
        assert_eq!(read(&to), "old data");
        // The next launch: nothing is left to carry.
        assert_eq!(carry(&from, &to, false, &[]), None);
        assert_eq!(read(&to), "old data");
    }

    #[test]
    fn both_present_and_set_up_leaves_both_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let (from, to) = (temp.path().join("old"), temp.path().join("new"));
        put(&from, "old data");
        put(&to, "new data");
        assert_eq!(carry(&from, &to, true, &[]), Some(Outcome::Kept));
        assert_eq!((read(&from).as_str(), read(&to).as_str()), ("old data", "new data"));
        // An empty new folder is still the app's own when the install is set up (elsewhere).
        fs::remove_file(to.join("settings")).unwrap();
        assert_eq!(carry(&from, &to, true, &[]), Some(Outcome::Kept));
        assert_eq!(read(&from), "old data");
        assert!(to.is_dir());
    }

    #[test]
    fn both_present_and_never_set_up_puts_the_old_one_in_place_of_the_first_start_folder() {
        let temp = tempfile::tempdir().unwrap();
        let (from, to) = (temp.path().join("old"), temp.path().join("new"));
        put(&from, "old data");
        put(&to, "first start");
        assert_eq!(carry(&from, &to, false, &[]), Some(Outcome::Replaced));
        assert!(!from.exists());
        assert_eq!(read(&to), "old data");
        let names: Vec<_> = fs::read_dir(temp.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("new")], "no folder is left set aside");
    }

    #[test]
    fn neither_present_is_nothing_to_carry() {
        let temp = tempfile::tempdir().unwrap();
        let (from, to) = (temp.path().join("old"), temp.path().join("new"));
        assert_eq!(carry(&from, &to, false, &[]), None);
        assert!(!from.exists() && !to.exists());
        // A new folder alone is the normal state of a fresh install.
        put(&to, "new data");
        assert_eq!(carry(&from, &to, false, &[]), None);
        assert_eq!(read(&to), "new data");
    }

    #[test]
    fn a_failed_move_is_reported_and_leaves_the_old_folder_in_place() {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("old");
        // No parent to rename into.
        let to = temp.path().join("missing").join("new");
        put(&from, "old data");
        assert!(matches!(carry(&from, &to, false, &[]), Some(Outcome::Failed(_))));
        assert_eq!(read(&from), "old data");
        assert!(!to.exists());
    }

    #[test]
    fn a_failed_replacement_puts_the_first_start_folder_back_and_keeps_the_old_one() {
        let temp = tempfile::tempdir().unwrap();
        let (from, to) = (temp.path().join("old"), temp.path().join("new"));
        put(&from, "old data");
        put(&to, "first start");
        let outcome = replace(&from, &to, &[], |source, target| {
            if source == from {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            } else {
                fs::rename(source, target)
            }
        });
        assert_eq!(outcome, Outcome::Failed(io::ErrorKind::PermissionDenied));
        assert_eq!((read(&from).as_str(), read(&to).as_str()), ("old data", "first start"));
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2, "nothing is left set aside");
    }

    #[test]
    fn the_install_is_set_up_by_what_the_person_did_and_not_by_a_first_start() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        assert!(!configured(&data), "no folder yet");
        fs::create_dir_all(data.join("logs")).unwrap();
        fs::write(data.join("logs").join("sidevoice-desktop.log"), "launch").unwrap();
        fs::write(data.join(MARKER), "").unwrap();
        fs::write(data.join(store::FILE_NAME), "{}").unwrap();
        assert!(!configured(&data), "a first start writes its log, pairs with a local core, and the marker is ours");
        fs::remove_file(data.join(store::FILE_NAME)).unwrap();
        for name in [settings::FILE_NAME, PLACEMENTS_FILE, PAIRINGS_FILE] {
            fs::write(data.join(name), "{}").unwrap();
            assert!(configured(&data), "{name}");
            fs::remove_file(data.join(name)).unwrap();
        }
        assert!(!configured(&data));
    }

    #[test]
    fn a_failure_does_not_stop_the_next_folder_and_every_folder_gets_one_line() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // `failing` cannot be moved: its destination is below a file.
        fs::write(root.join("blocker"), "a file").unwrap();
        let pair = |from: &str, to: PathBuf| (root.join(from), to);
        let moves = Moves {
            data: pair("old-data", root.join("new-data")),
            others: vec![
                pair("old-failing", root.join("blocker/new-failing")),
                pair("old-moving", root.join("new-moving")),
                pair("old-kept", root.join("new-kept")),
                pair("old-absent", root.join("new-absent")),
            ],
        };
        put(&moves.data.0, "data");
        put(&moves.others[0].0, "failing");
        put(&moves.others[1].0, "moving");
        put(&moves.others[2].0, "old kept");
        put(&moves.others[2].1, "new kept");

        let mut report = Report(Vec::new());
        assert!(!folders(&moves, true, &mut report));

        assert_eq!(read(&moves.others[0].0), "failing");
        assert_eq!(read(&moves.others[1].1), "moving");
        assert!(!moves.others[1].0.exists());
        assert_eq!((read(&moves.others[2].0).as_str(), read(&moves.others[2].1).as_str()), ("old kept", "new kept"));
        let levels: Vec<Level> = report.0.iter().map(|(level, _)| *level).collect();
        assert_eq!(levels, vec![Level::Warn, Level::Info, Level::Info], "one line each, none for the absent");
        assert!(report.0[1].1.starts_with("moved "));
        assert!(report.0[2].1.starts_with("kept "));
        // The app data waits for a retry in which the rest went through: it is what `configured` is read from.
        assert_eq!(read(&moves.data.0), "data");
        assert!(!moves.data.1.exists());
    }

    /// An install laid out in a temporary folder: the old and the new app data, and one other folder each.
    fn install(root: &Path) -> Moves {
        Moves {
            data: (root.join("old-data"), root.join("new-data")),
            others: vec![(root.join("old-web"), root.join("new-web"))],
        }
    }

    fn no_keys() -> Result<credentials::Carried, Error> {
        Ok(Vec::new())
    }

    fn marked(moves: &Moves) -> bool {
        moves.data.1.join(MARKER).exists()
    }

    #[test]
    fn a_fresh_new_install_next_to_old_data_is_replaced_by_it_and_marked() {
        // The nightly that shipped the new identifier with no migration: it started once and wrote its log.
        let temp = tempfile::tempdir().unwrap();
        let moves = install(temp.path());
        put(&moves.data.0, "old settings");
        fs::write(moves.data.0.join(settings::FILE_NAME), "{}").unwrap();
        put(&moves.others[0].0, "old web");
        fs::create_dir_all(moves.data.1.join("logs")).unwrap();
        put(&moves.others[0].1, "first-start web");

        let report = run(&moves, || false, no_keys);

        assert_eq!(read(&moves.data.1), "old settings");
        assert!(moves.data.1.join(settings::FILE_NAME).exists());
        assert!(!moves.data.1.join("logs").exists(), "the first-start folder is gone");
        assert_eq!(read(&moves.others[0].1), "old web");
        assert!(!moves.data.0.exists() && !moves.others[0].0.exists());
        assert!(marked(&moves));
        let lines: Vec<_> = report.0.iter().map(|(level, line)| (*level, line.as_str())).collect();
        assert_eq!(lines.len(), 2, "one line per folder");
        assert!(lines.iter().all(|(level, line)| *level == Level::Info && line.contains("replacing")));

        // The next launch finds the marker and does nothing, even with a new "old" folder.
        put(&moves.others[0].0, "another old web");
        assert!(run(&moves, || false, no_keys).0.is_empty());
        assert_eq!(read(&moves.others[0].0), "another old web");
    }

    #[test]
    fn the_first_start_pairing_with_a_local_core_outlives_the_replacement() {
        // The core revoked the old install's token when the new build paired: the new file is the live one.
        let temp = tempfile::tempdir().unwrap();
        let moves = install(temp.path());
        put(&moves.data.0, "old settings");
        fs::write(moves.data.0.join(settings::FILE_NAME), "{}").unwrap();
        fs::write(moves.data.0.join(store::FILE_NAME), "revoked token").unwrap();
        put(&moves.data.1, "first start");
        fs::write(moves.data.1.join(store::FILE_NAME), "live token").unwrap();

        run(&moves, || false, no_keys);

        assert_eq!(read(&moves.data.1), "old settings");
        assert!(moves.data.1.join(settings::FILE_NAME).exists());
        assert_eq!(fs::read_to_string(moves.data.1.join(store::FILE_NAME)).unwrap(), "live token");
        assert!(marked(&moves));
    }
    #[test]
    fn two_set_up_installs_keep_the_new_one_leave_the_old_one_and_are_marked() {
        let temp = tempfile::tempdir().unwrap();
        let moves = install(temp.path());
        put(&moves.data.0, "old settings");
        put(&moves.data.1, "new settings");
        fs::write(moves.data.1.join(PAIRINGS_FILE), "{}").unwrap();
        put(&moves.others[0].0, "old web");
        put(&moves.others[0].1, "new web");

        let report = run(&moves, || false, no_keys);

        assert_eq!((read(&moves.data.0).as_str(), read(&moves.data.1).as_str()), ("old settings", "new settings"));
        assert_eq!((read(&moves.others[0].0).as_str(), read(&moves.others[0].1).as_str()), ("old web", "new web"));
        assert!(marked(&moves), "decided: not asked again");
        assert_eq!(report.0.len(), 2);
        assert!(report.0.iter().all(|(level, line)| *level == Level::Info && line.starts_with("kept ")));
    }

    #[test]
    fn a_provider_key_in_the_new_namespace_makes_the_install_set_up() {
        let temp = tempfile::tempdir().unwrap();
        let moves = install(temp.path());
        put(&moves.data.0, "old settings");
        put(&moves.data.1, "first start");
        run(&moves, || true, no_keys);
        assert_eq!((read(&moves.data.0).as_str(), read(&moves.data.1).as_str()), ("old settings", "first start"));
        assert!(marked(&moves));
    }

    #[test]
    fn an_old_install_moves_in_where_nothing_has_run_and_nothing_to_carry_is_marked() {
        let temp = tempfile::tempdir().unwrap();
        let moves = install(temp.path());
        put(&moves.data.0, "old settings");
        run(&moves, || false, no_keys);
        assert_eq!(read(&moves.data.1), "old settings");
        assert!(marked(&moves));

        // A fresh install: no old data at all, so only the marker is written (creating the folder).
        let fresh = tempfile::tempdir().unwrap();
        let moves = install(fresh.path());
        assert!(run(&moves, || false, no_keys).0.is_empty());
        assert!(marked(&moves));
        assert!(!moves.others[0].1.exists());
    }

    #[test]
    fn a_step_that_failed_leaves_no_marker_so_the_next_launch_tries_again() {
        let temp = tempfile::tempdir().unwrap();
        let moves = install(temp.path());
        put(&moves.data.0, "old settings");
        // A key that cannot be moved: the folders are carried, the marker waits.
        let report = run(&moves, || false, || Ok(vec![("openai".into(), Err(Error::Failed))]));
        assert_eq!(read(&moves.data.1), "old settings");
        assert!(!marked(&moves));
        assert!(report.0.iter().any(|(level, line)| *level == Level::Warn && line.contains("openai")));

        // A store that cannot be listed.
        let report = run(&moves, || false, || Err(Error::Unavailable));
        assert!(!marked(&moves));
        assert!(report.0.iter().any(|(level, line)| *level == Level::Warn && line.contains("not looked at")));

        // The retry: the keys go through.
        let report = run(&moves, || false, || Ok(vec![("openai".into(), Ok(true)), ("other".into(), Ok(false))]));
        assert!(marked(&moves));
        assert_eq!(report.0.len(), 2, "one line per key");
        assert_eq!(read(&moves.data.1), "old settings");
    }
}
