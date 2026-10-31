//! One-off carry-over of an install's data from the bundle identifier the app had before `ai.sidevoice.desktop`
//! (`dev.sidevoice.desktop`), so a person who updates keeps their settings, pairing, browser storage and provider keys.
//! It also serves the builds that already ran under the new identifier with no migration (the nightly of #316): their
//! folders are first-start leftovers, and the old install's data is still where it was.
//!
//! KEEP THIS MODULE UNTIL 1.0 (RELEASING.md, "Legacy identifier carry-over"); removing it needs a release note that
//! says "update through vX.Y first". What exists only for it: `platform::app_data::{moves_from, Moves}` and its trait
//! method `extra_folders`, `platform::webview_data::locations_of`, `platform::credentials::{carry_over, holds_keys}`,
//! `data::PAIRINGS_FILE` unless something else uses it, and the `stop` call in the reset helper (`data.rs`).
//!
//! [`carry_over`] runs at every launch, first, under the launch lock (`reset_handoff::Handoff::await_idle`: no other
//! launch, helper or reset runs meanwhile), before the log, the app data, the webview store or a credential is
//! touched. That lock ends with startup, so it also takes the **instance lock** for the life of the process
//! (`hold_instance`): a launch that cannot is a second instance, whose running first may have its browser store open,
//! and carries nothing. (The single-instance plugin is registered after the log plugin, whose setup opens the log file
//! in folders the migration moves, and Windows cannot rename a folder with an open file in it: so the migration cannot
//! wait for that plugin to decide.)
//!
//! Its state is a **journal** in the reset's control directory (`legacy-identifier.json`), which no reset deletes:
//! whether the install was `configured`, decided once; how far each step got; whether it is finished. A launch resumes
//! from it and never decides again, so a partial failure (a locked browser folder while the keys moved) cannot change
//! the outcome of the retry. A **reset** ends the carry-over (`stop`), finished or not, so nothing it deletes comes
//! back from the old folders. A step writes its phase before it acts and every phase is idempotent, so a crash
//! anywhere resumes where it was.
//!
//! - **Folders**: each folder the app keeps for the old identifier (`app_data::moves_from`: the reset's own folder
//!   list, so the app data with the local-host pairing in it, the browser storage, the cache and, on macOS, the logs
//!   and saved window state) that exists is carried to the new identifier's.
//!   - The new one is absent: the old one is renamed to it (siblings: a rename on one filesystem).
//!   - The new one exists and the install is `configured`: no settings, remote pairings or card placement file in
//!     the new app data folder and no provider key in the new namespace means it was never set up. Set up: the new
//!     one stays, the old one is left in place and logged. Two installs are never merged.
//!   - The new one exists and the install was never set up: the new one is renamed to `…first-start-backup` next to
//!     it, never deleted (the web interface keeps preferences in its storage that no file shows), and the old one
//!     renamed into place. The local host's pairing (`local-host.json`) is made on its own with a core running here,
//!     and revoked the old install's, so the backup's copy of it is copied over the old one's before the step counts as
//!     done.
//!   - Unknown whether it was set up (the key store could not be reached): nothing is replaced and no key moved; the
//!     decision is asked again at the next launch.
//! - **Provider keys**: each old key without a new one is copied to the new namespace, read back, and deleted from the
//!   old one (`credentials::carry_over`); a key the new namespace holds already wins.
//! - **Finished** when every step is done (nothing to carry included). A step that failed is retried at the next
//!   launch.
//!
//! It never fails the launch and never deletes old data it did not first have somewhere else. Not carried: macOS's
//! `defaults` domain (system UI state), the macOS microphone grant (the system keys it by bundle id) and the reset's
//! control directory.

use crate::data::PAIRINGS_FILE;
use crate::platform::app_data::{self, Moves};
use crate::platform::{account, credentials, reset_handoff};
use log::Level;
use serde::{Deserialize, Serialize};
use sidevoice_desktop_core::call_controls::PLACEMENTS_FILE;
use sidevoice_desktop_core::settings;
use sidevoice_local_host::store;
use sidevoice_platform::fs::{Create, Files, Fs};
use std::{
    cell::Cell,
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::OnceLock,
};

/// The bundle identifier of the builds before `ai.sidevoice.desktop`.
const LEGACY: &str = "dev.sidevoice.desktop";

/// The journal, in the reset's control directory.
const JOURNAL: &str = "legacy-identifier.json";

/// The lock a running instance holds, in the same directory.
const INSTANCE_LOCK: &str = "instance.lock";

/// The step of the provider keys, in the journal beside the folders' (named by their new path).
const KEYS: &str = "keys";

/// How far a step got, written before the work of that phase is done.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    /// About to set the first-start folder aside.
    Aside,
    /// About to move the old folder into place, and to copy over what outlives the first start.
    In,
    Done,
}

/// The journal's content.
#[derive(Default, Serialize, Deserialize)]
struct State {
    /// Nothing more is done: the carry-over ended, or a reset ended it.
    finished: bool,
    /// Whether the install was set up, decided once; `None` while the key store could not say.
    configured: Option<bool>,
    steps: BTreeMap<String, Phase>,
}

fn read_state(control: &Path) -> io::Result<State> {
    match fs::read(control.join(JOURNAL)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::from),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(State::default()),
        Err(error) => Err(error),
    }
}

fn write_state(control: &Path, state: &State) -> io::Result<()> {
    Files::write_private(&control.join(JOURNAL), &serde_json::to_vec(state)?)
}

/// A reset ends the carry-over, finished or not: what it deletes must not come back from the old identifier's folders.
/// `control` is the reset's control directory.
pub fn stop(control: &Path) -> io::Result<()> {
    write_state(control, &State { finished: true, ..State::default() })
}

/// The calls that change the filesystem, in one place so a test can make one fail (`fail_at`), or that one and every
/// one after it as a crash does (`crash_at`).
#[derive(Default)]
struct Io {
    calls: Cell<usize>,
    fail_at: Option<usize>,
    crash_at: Option<usize>,
}

impl Io {
    fn step(&self) -> io::Result<()> {
        let call = self.calls.get();
        self.calls.set(call + 1);
        if self.fail_at == Some(call) || self.crash_at.is_some_and(|from| call >= from) {
            return Err(io::Error::other("injected failure"));
        }
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.step()?;
        fs::rename(from, to)
    }

    fn copy(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.step()?;
        fs::copy(from, to).map(drop)
    }
}

/// The state and where it is kept; every change is written at once.
struct Journal<'a> {
    control: &'a Path,
    io: &'a Io,
    state: State,
}

impl Journal<'_> {
    fn save(&self) -> io::Result<()> {
        self.io.step()?;
        write_state(self.control, &self.state)
    }

    fn set(&mut self, step: &str, phase: Phase) -> io::Result<()> {
        self.state.steps.insert(step.to_owned(), phase);
        self.save()
    }
}

/// What happened to one old folder that exists.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Renamed to where the app keeps it now.
    Moved,
    /// The old one took the place of the first-start folder, which is kept at this path.
    Replaced(PathBuf),
    /// Left in place: the app already has a set-up folder of its own there, and two are never merged.
    Kept,
    /// Left for a later launch: not known whether the install was set up.
    Deferred,
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
    let said = |line: String| Report(vec![(Level::Info, line)]);
    let Ok(control) = reset_handoff::state_dir() else { return said("no control directory: not looked at".into()) };
    if !hold_instance(&control) {
        return said("another instance is running: left for a later launch".into());
    }
    match account::home().and_then(|home| app_data::moves_from(&home, LEGACY)) {
        Ok(moves) => {
            let keys_held = || credentials::holds_keys().ok();
            run(&moves, &control, &Io::default(), keys_held, || credentials::carry_over(LEGACY))
        }
        Err(key) => said(format!("not looked at ({key})")),
    }
}

/// Take, for the life of this process, the lock only a running instance holds (the OS releases it when the process
/// ends, however it ends). False when another instance holds it.
fn hold_instance(control: &Path) -> bool {
    static HELD: OnceLock<fs::File> = OnceLock::new();
    let Some(file) = lock(control) else { return false };
    let _ = HELD.set(file);
    true
}

fn lock(control: &Path) -> Option<fs::File> {
    let file = Files::open_private(&control.join(INSTANCE_LOCK), Create::ReadWrite).ok()?;
    file.try_lock().ok().map(|()| file)
}

/// [`carry_over`] for these folders, with the journal in `control`. `keys_held` says whether the new namespace holds a
/// provider key (`None`: the store could not say); `carry_keys` carries the old keys.
fn run(
    moves: &Moves,
    control: &Path,
    io: &Io,
    keys_held: impl FnOnce() -> Option<bool>,
    carry_keys: impl FnOnce() -> Result<credentials::Carried, credentials::Error>,
) -> Report {
    let mut report = Report(Vec::new());
    let state = match read_state(control) {
        Ok(state) if state.finished => return report,
        Ok(state) => state,
        Err(error) => {
            report.0.push((Level::Warn, format!("the journal cannot be read ({}): nothing done", error.kind())));
            return report;
        }
    };
    let mut journal = Journal { control, io, state };
    if journal.state.configured.is_none() {
        // Once, before anything moves or a key is copied (both change what this looks at).
        journal.state.configured = if configured(&moves.data.1) { Some(true) } else { keys_held() };
        if journal.state.configured.is_none() {
            report.0.push((Level::Info, "the key store could not say whether the install was set up".into()));
        } else if let Err(error) = journal.save() {
            report.0.push((Level::Warn, format!("the journal cannot be written ({}): nothing done", error.kind())));
            return report;
        }
    }
    let folders_settled = folders(moves, &mut journal, &mut report);
    let keys_settled = keys(&mut journal, carry_keys, &mut report);
    if folders_settled && keys_settled {
        journal.state.finished = true;
        if let Err(error) = journal.save() {
            report.0.push((Level::Warn, format!("the end could not be written ({})", error.kind())));
        }
    }
    report
}

/// Carry each `(old, new)` folder, reporting every one that exists; whether all are settled. A failure does not stop
/// the next folder, but the app data goes last and only when the rest are settled: `configured` was read from it.
fn folders(moves: &Moves, journal: &mut Journal, report: &mut Report) -> bool {
    let mut settled = true;
    for (from, to) in &moves.others {
        settled &= step(from, to, &[], journal, report);
    }
    settled && step(&moves.data.0, &moves.data.1, &[store::FILE_NAME], journal, report)
}

/// One folder: carried, reported; whether it is settled.
fn step(from: &Path, to: &Path, keep: &[&str], journal: &mut Journal, report: &mut Report) -> bool {
    let outcome = carry(from, to, keep, journal).unwrap_or_else(|error| Some(Outcome::Failed(error.kind())));
    let (from, to) = (from.display(), to.display());
    let (settled, line) = match outcome {
        None => return true,
        Some(Outcome::Moved) => (true, (Level::Info, format!("moved {from} to {to}"))),
        Some(Outcome::Replaced(backup)) => {
            let kept = backup.display();
            (true, (Level::Info, format!("moved {from} to {to}; what a first start had made there is kept as {kept}")))
        }
        Some(Outcome::Kept) => (true, (Level::Info, format!("kept {from}: {to} already exists and is set up"))),
        Some(Outcome::Deferred) => {
            (false, (Level::Info, format!("left {from}: not known whether {to} is set up, tried again later")))
        }
        Some(Outcome::Failed(kind)) => (false, (Level::Warn, format!("could not move {from} to {to}: {kind}"))),
    };
    report.0.push(line);
    settled
}

/// The move rule, resumable. `None` when there is nothing to carry. A phase is written before its work, and its work
/// looks at what is there, so a crash anywhere resumes: the first-start folder is set aside only if it is still in
/// place, the old one moved only if it is still there, `keep`'s files copied over again. The first-start folder is
/// never deleted, and `keep`'s files are the ones that outlive it (`local-host.json`): the step is done only after
/// they are copied. An error with the first-start folder set aside and nothing in its place puts it back, so the app
/// starts on it as before and the step starts over.
fn carry(from: &Path, to: &Path, keep: &[&str], journal: &mut Journal) -> io::Result<Option<Outcome>> {
    let id = to.to_string_lossy().into_owned();
    match journal.state.steps.get(&id).copied() {
        Some(Phase::Done) => return Ok(None),
        Some(_) => (),
        None if !exists(from)? => {
            journal.set(&id, Phase::Done)?;
            return Ok(None);
        }
        None if !exists(to)? => journal.set(&id, Phase::In)?,
        None => match journal.state.configured {
            Some(true) => {
                journal.set(&id, Phase::Done)?;
                return Ok(Some(Outcome::Kept));
            }
            None => return Ok(Some(Outcome::Deferred)),
            Some(false) => journal.set(&id, Phase::Aside)?,
        },
    }
    let backup = backup_of(to);
    if let Err(error) = relocate(from, to, &backup, keep, &id, journal) {
        if exists(&backup).unwrap_or(false) && !exists(to).unwrap_or(true) && journal.io.rename(&backup, to).is_ok() {
            journal.state.steps.remove(&id);
            let _ = journal.save();
        }
        return Err(error);
    }
    Ok(Some(if exists(&backup)? { Outcome::Replaced(backup) } else { Outcome::Moved }))
}

/// The phases of a started step: set aside, move in, copy over what outlives the first start, done.
fn relocate(from: &Path, to: &Path, backup: &Path, keep: &[&str], id: &str, journal: &mut Journal) -> io::Result<()> {
    if journal.state.steps[id] == Phase::Aside {
        if exists(to)? && !exists(backup)? {
            journal.io.rename(to, backup)?;
        }
        journal.set(id, Phase::In)?;
    }
    if exists(from)? {
        journal.io.rename(from, to)?;
    }
    for name in keep {
        let kept = backup.join(name);
        if exists(&kept)? {
            journal.io.copy(&kept, &to.join(name))?;
        }
    }
    journal.set(id, Phase::Done)
}

/// The provider keys, once the install's decision is known (carrying them changes what that looks at).
fn keys(
    journal: &mut Journal,
    carry_keys: impl FnOnce() -> Result<credentials::Carried, credentials::Error>,
    report: &mut Report,
) -> bool {
    if journal.state.steps.get(KEYS) == Some(&Phase::Done) {
        return true;
    }
    if journal.state.configured.is_none() {
        return false;
    }
    let carried = match carry_keys() {
        Ok(carried) => carried,
        Err(error) => {
            report.0.push((Level::Warn, format!("provider keys not looked at ({error:?})")));
            return false;
        }
    };
    let mut settled = true;
    for (provider, result) in carried {
        report.0.push(match result {
            Ok(true) => (Level::Info, format!("moved the {provider} key to the new namespace")),
            Ok(false) => (Level::Info, format!("removed the old {provider} key: the new namespace already holds one")),
            Err(error) => {
                settled = false;
                (Level::Warn, format!("could not move the {provider} key: {error:?}"))
            }
        });
    }
    if !settled {
        return false;
    }
    match journal.set(KEYS, Phase::Done) {
        Ok(()) => true,
        Err(error) => {
            report.0.push((Level::Warn, format!("the journal cannot be written ({})", error.kind())));
            false
        }
    }
}

/// Where a first-start folder is kept when the old one takes its place: next to it, never deleted.
fn backup_of(to: &Path) -> PathBuf {
    let mut name = to.file_name().unwrap_or_default().to_os_string();
    name.push(".first-start-backup");
    to.with_file_name(name)
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

#[cfg(test)]
mod tests {
    use super::*;
    use credentials::Error;
    use sidevoice_platform::fs::TestFiles;

    /// A private scratch folder with a private control directory in it.
    fn scratch() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::Builder::new().tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        Files::shut_others_out(temp.path());
        let root = dunce::canonicalize(temp.path()).unwrap();
        Files::create_private_dir(&root.join("control")).unwrap();
        (temp, root)
    }

    fn put(dir: &Path, name: &str, text: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), text).unwrap();
    }

    fn read(dir: &Path, name: &str) -> String {
        fs::read_to_string(dir.join(name)).unwrap()
    }

    fn no_keys() -> Result<credentials::Carried, Error> {
        Ok(Vec::new())
    }

    /// The old install and the first start of the new one: what the nightly of #316 left.
    fn nightly(root: &Path) -> Moves {
        let moves = Moves {
            data: (root.join("old-data"), root.join("new-data")),
            others: vec![(root.join("old-web"), root.join("new-web"))],
        };
        put(&moves.data.0, "note", "old settings");
        put(&moves.data.0, settings::FILE_NAME, "{}");
        put(&moves.data.0, store::FILE_NAME, "revoked token");
        put(&moves.others[0].0, "note", "old web");
        put(&moves.data.1.join("logs"), "log", "launch");
        put(&moves.data.1, store::FILE_NAME, "live token");
        put(&moves.others[0].1, "note", "first-start web");
        moves
    }

    fn healthy(moves: &Moves, control: &Path) -> Report {
        run(moves, control, &Io::default(), || Some(false), no_keys)
    }

    /// Everything of the old install is in place, the live token with it, and the first start is kept.
    fn assert_carried(moves: &Moves, control: &Path) {
        assert_eq!(read(&moves.data.1, "note"), "old settings");
        assert!(moves.data.1.join(settings::FILE_NAME).exists());
        assert_eq!(read(&moves.data.1, store::FILE_NAME), "live token");
        assert_eq!(read(&moves.others[0].1, "note"), "old web");
        assert!(!moves.data.0.exists() && !moves.others[0].0.exists());
        assert!(read_state(control).unwrap().finished);
    }

    /// Whether `text` is in `name` of one of `dirs`.
    fn somewhere(dirs: &[&PathBuf], name: &str, text: &str) -> bool {
        dirs.iter().any(|dir| fs::read_to_string(dir.join(name)).is_ok_and(|found| found == text))
    }

    #[test]
    fn a_fresh_new_install_next_to_old_data_is_replaced_by_it_and_its_first_start_is_kept() {
        let (_temp, root) = scratch();
        let (moves, control) = (nightly(&root), root.join("control"));

        let report = healthy(&moves, &control);

        assert_carried(&moves, &control);
        assert_eq!(read(&root.join("new-data.first-start-backup"), store::FILE_NAME), "live token");
        assert_eq!(read(&root.join("new-web.first-start-backup"), "note"), "first-start web");
        assert!(root.join("new-data.first-start-backup/logs/log").exists());
        let lines: Vec<_> = report.0.iter().map(|(level, line)| (*level, line.as_str())).collect();
        assert_eq!(lines.len(), 2, "one line per folder");
        assert!(lines.iter().all(|(level, line)| *level == Level::Info && line.contains("first-start-backup")));

        // Finished: a folder that shows up later is not carried.
        put(&moves.others[0].0, "note", "another old web");
        assert!(healthy(&moves, &control).0.is_empty());
        assert_eq!(read(&moves.others[0].0, "note"), "another old web");
    }

    #[test]
    fn two_set_up_installs_keep_the_new_one_leave_the_old_one_and_finish() {
        let (_temp, root) = scratch();
        let (moves, control) = (nightly(&root), root.join("control"));
        put(&moves.data.1, PAIRINGS_FILE, "{}");

        let report = run(&moves, &control, &Io::default(), || unreachable!("the files say so"), no_keys);

        assert_eq!(read(&moves.data.0, "note"), "old settings");
        assert!(!moves.data.1.join("note").exists());
        assert_eq!(read(&moves.others[0].0, "note"), "old web");
        assert_eq!(read(&moves.others[0].1, "note"), "first-start web");
        assert!(!root.join("new-data.first-start-backup").exists());
        assert!(read_state(&control).unwrap().finished);
        assert_eq!(report.0.len(), 2);
        assert!(report.0.iter().all(|(level, line)| *level == Level::Info && line.starts_with("kept ")));
    }

    #[test]
    fn a_provider_key_in_the_new_namespace_makes_the_install_set_up() {
        let (_temp, root) = scratch();
        let (moves, control) = (nightly(&root), root.join("control"));
        run(&moves, &control, &Io::default(), || Some(true), no_keys);
        assert_eq!(read(&moves.data.0, "note"), "old settings");
        assert!(!moves.data.1.join("note").exists());
        assert!(read_state(&control).unwrap().finished);
    }

    #[test]
    fn an_old_install_moves_in_where_nothing_has_run_and_a_fresh_install_just_finishes() {
        let (_temp, root) = scratch();
        let control = root.join("control");
        let moves = Moves { data: (root.join("old-data"), root.join("new-data")), others: Vec::new() };
        put(&moves.data.0, "note", "old settings");
        healthy(&moves, &control);
        assert_eq!(read(&moves.data.1, "note"), "old settings");
        assert!(!root.join("new-data.first-start-backup").exists());
        assert!(read_state(&control).unwrap().finished);

        let (_fresh, root) = scratch();
        let moves = Moves { data: (root.join("old-data"), root.join("new-data")), others: Vec::new() };
        assert!(healthy(&moves, &root.join("control")).0.is_empty());
        assert!(!moves.data.1.exists(), "nothing is created");
        assert!(read_state(&root.join("control")).unwrap().finished);
    }

    #[test]
    fn every_crash_point_loses_nothing_and_the_next_launch_finishes() {
        for crash_at in 0.. {
            let (_temp, root) = scratch();
            let (moves, control) = (nightly(&root), root.join("control"));
            let io = Io { crash_at: Some(crash_at), ..Io::default() };
            run(&moves, &control, &io, || Some(false), no_keys);
            let crashed = io.calls.get() > crash_at;

            // Wherever it stopped, the old install and the live token are somewhere.
            let (old, new, back) = (&moves.data.0, &moves.data.1, root.join("new-data.first-start-backup"));
            assert!(somewhere(&[old, new], "note", "old settings"), "crash at {crash_at}");
            assert!(somewhere(&[new, &back], store::FILE_NAME, "live token"), "crash at {crash_at}");
            let (old, new) = (&moves.others[0].0, &moves.others[0].1);
            assert!(somewhere(&[old, new], "note", "old web"), "crash at {crash_at}");

            // The next launch: it is not a new decision, and it finishes.
            healthy(&moves, &control);
            assert_carried(&moves, &control);
            if !crashed {
                break;
            }
        }
    }

    #[test]
    fn a_failed_step_is_retried_with_the_original_decision_even_after_the_keys_moved() {
        for fail_at in 0.. {
            let (_temp, root) = scratch();
            let (moves, control) = (nightly(&root), root.join("control"));
            // Carrying the keys is what makes the new namespace hold one.
            let held = Cell::new(false);
            let carry_keys = || {
                held.set(true);
                Ok(vec![("openai".to_owned(), Ok(true))])
            };
            let io = Io { fail_at: Some(fail_at), ..Io::default() };
            run(&moves, &control, &io, || Some(held.get()), carry_keys);
            let failed = io.calls.get() > fail_at;

            // The app starts between the launches, on whatever folder is there.
            for new in [&moves.data.1, &moves.others[0].1] {
                if !new.exists() {
                    put(new, "note", "first start");
                }
            }
            run(&moves, &control, &Io::default(), || Some(held.get()), carry_keys);
            assert_carried(&moves, &control);
            if !failed {
                break;
            }
        }
    }

    #[test]
    fn an_unavailable_key_store_replaces_nothing_and_is_asked_again() {
        let (_temp, root) = scratch();
        let (moves, control) = (nightly(&root), root.join("control"));
        // The browser folder has no new counterpart yet: moving it needs no decision.
        fs::remove_dir_all(&moves.others[0].1).unwrap();

        let report = run(&moves, &control, &Io::default(), || None, || unreachable!("no key moves while unknown"));

        assert_eq!(read(&moves.others[0].1, "note"), "old web");
        assert_eq!(read(&moves.data.0, "note"), "old settings", "not replaced");
        assert_eq!(read(&moves.data.1, store::FILE_NAME), "live token");
        assert!(!root.join("new-data.first-start-backup").exists());
        let state = read_state(&control).unwrap();
        assert!(!state.finished && state.configured.is_none());
        assert!(report.0.iter().any(|(_, line)| line.contains("could not say")));

        // The store is back: the decision is made and the rest goes through.
        run(&moves, &control, &Io::default(), || Some(false), no_keys);
        assert_carried(&moves, &control);
    }

    #[test]
    fn a_reset_ends_the_carry_over_before_it_started_and_halfway() {
        // Before: nothing is carried afterwards.
        let (_temp, root) = scratch();
        let (moves, control) = (nightly(&root), root.join("control"));
        stop(&control).unwrap();
        assert!(healthy(&moves, &control).0.is_empty());
        assert_eq!(read(&moves.data.0, "note"), "old settings");
        assert_eq!(read(&moves.data.1, store::FILE_NAME), "live token");

        // Halfway, wherever it stopped: the reset deletes the new folders, and nothing comes back.
        for crash_at in 0..16 {
            let (_temp, root) = scratch();
            let (moves, control) = (nightly(&root), root.join("control"));
            run(&moves, &control, &Io { crash_at: Some(crash_at), ..Io::default() }, || Some(false), no_keys);
            for new in [&moves.data.1, &moves.others[0].1, &root.join("new-data.first-start-backup")] {
                let _ = fs::remove_dir_all(new);
            }
            let _ = fs::remove_dir_all(root.join("new-web.first-start-backup"));
            stop(&control).unwrap();
            healthy(&moves, &control);
            assert!(!moves.data.1.exists() && !moves.others[0].1.exists(), "after a crash at {crash_at}");
        }
    }

    #[test]
    fn a_reset_after_the_carry_over_does_not_bring_a_kept_old_folder_back() {
        let (_temp, root) = scratch();
        let (moves, control) = (nightly(&root), root.join("control"));
        put(&moves.data.1, PAIRINGS_FILE, "{}");
        healthy(&moves, &control);
        assert_eq!(read(&moves.data.0, "note"), "old settings", "kept");

        // The reset deletes the app data; the journal in the control directory is not touched, and the reset ends it.
        fs::remove_dir_all(&moves.data.1).unwrap();
        stop(&control).unwrap();
        assert!(healthy(&moves, &control).0.is_empty());
        assert!(!moves.data.1.exists());
    }

    #[test]
    fn a_journal_that_cannot_be_read_does_nothing() {
        let (_temp, root) = scratch();
        let (moves, control) = (nightly(&root), root.join("control"));
        fs::write(control.join(JOURNAL), "{ not json").unwrap();
        let report = healthy(&moves, &control);
        assert_eq!(report.0.len(), 1);
        assert_eq!(read(&moves.data.0, "note"), "old settings");
        assert_eq!(read(&moves.data.1, store::FILE_NAME), "live token");
    }

    #[test]
    fn only_one_instance_holds_the_lock_until_it_ends() {
        let (_temp, root) = scratch();
        let control = root.join("control");
        let first = lock(&control).expect("the first instance");
        assert!(lock(&control).is_none(), "a second one does not carry anything");
        drop(first);
        assert!(lock(&control).is_some());
    }

    #[test]
    fn the_install_is_set_up_by_what_the_person_did_and_not_by_a_first_start() {
        let (_temp, root) = scratch();
        let data = root.join("data");
        assert!(!configured(&data), "no folder yet");
        put(&data.join("logs"), "sidevoice-desktop.log", "launch");
        put(&data, store::FILE_NAME, "{}");
        assert!(!configured(&data), "a first start writes its log, and pairs with a local core on its own");
        for name in [settings::FILE_NAME, PLACEMENTS_FILE, PAIRINGS_FILE] {
            fs::write(data.join(name), "{}").unwrap();
            assert!(configured(&data), "{name}");
            fs::remove_file(data.join(name)).unwrap();
        }
        assert!(!configured(&data));
    }

    #[test]
    fn a_failure_does_not_stop_the_next_folder_and_the_app_data_waits() {
        let (_temp, root) = scratch();
        let control = root.join("control");
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
        put(&moves.data.0, "note", "data");
        put(&moves.others[0].0, "note", "failing");
        put(&moves.others[1].0, "note", "moving");
        put(&moves.others[2].0, "note", "old kept");
        put(&moves.others[2].1, "note", "new kept");

        let io = Io::default();
        let state = State { configured: Some(true), ..State::default() };
        let mut journal = Journal { control: &control, io: &io, state };
        let mut report = Report(Vec::new());
        assert!(!folders(&moves, &mut journal, &mut report));

        assert_eq!(read(&moves.others[0].0, "note"), "failing");
        assert_eq!(read(&moves.others[1].1, "note"), "moving");
        assert!(!moves.others[1].0.exists());
        assert_eq!(read(&moves.others[2].0, "note"), "old kept");
        assert_eq!(read(&moves.others[2].1, "note"), "new kept");
        let levels: Vec<Level> = report.0.iter().map(|(level, _)| *level).collect();
        assert_eq!(levels, vec![Level::Warn, Level::Info, Level::Info], "one line each, none for the absent");
        assert_eq!(read(&moves.data.0, "note"), "data");
        assert!(!moves.data.1.exists());
    }
}
