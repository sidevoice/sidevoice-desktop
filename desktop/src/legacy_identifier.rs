//! One-off carry-over of an install's data from the bundle identifier the app had before `ai.sidevoice.desktop`
//! (`dev.sidevoice.desktop`), so a person who updates keeps their settings, pairings, browser storage and provider keys.
//! It also serves the builds that already ran under the new identifier with no carry-over (the nightly of #316): their
//! folders are first-start leftovers, and the old install's data is still where it was.
//!
//! KEEP THIS MODULE UNTIL 1.0 (RELEASING.md, "Legacy identifier carry-over"); removing it needs a release note that
//! says "update through vX.Y first". What exists only for it: `platform::app_data::carried_from`,
//! `platform::webview_data::locations_of`, `platform::credentials::copy_from`, `data::PAIRINGS_FILE` unless something
//! else uses it, and the `stop` call in `data::begin_reset`.
//!
//! **It copies and never deletes or moves anything of the old identifier**: the old folders and keys stay where they
//! are, orphaned (docs/APP_DATA.md says how to remove them by hand). Whatever happens here (a crash, a failure, a
//! retry, a reset), the old install is intact. On the new side nothing is deleted either: what a carried folder
//! replaces is kept next to it, and the only thing ever removed is a copy of this module's own that never went in.
//!
//! [`carry_over`] runs at every launch, first, under the launch lock (`reset_handoff::Handoff::await_idle`: no other
//! launch, helper or reset runs meanwhile), before the log, the app data, the webview store or a credential is
//! touched, until its journal (`legacy-identifier.json`, in the reset's control directory, which no reset deletes) says
//! it has finished. The launch lock ends with startup, so it also takes the **instance lock** for the life of the
//! process (`hold_instance`): a launch that cannot is a second instance, and carries nothing. A running build of the
//! old identifier is unaffected (nothing of it changes). A running build of the new identifier from before this
//! module (the nightly of #316) takes no lock: the updater quits the app before it installs, and a second process
//! started by hand meanwhile is not detected (docs/APP_DATA.md).
//!
//! - **Decided once**, before anything is carried, and written to the journal: whether the install here was set up
//!   (any of the settings, the card's placements or the remote pairings in its app data folder). If it was, no folder
//!   is touched.
//! - **Folders** (the app data and the browser storage, `app_data::carried_from`), each old one that exists: copied to
//!   `<new>.migrating` (a leftover of an earlier attempt removed first); the new folder's `local-host.json`, the live
//!   token of the local host (pairing the first start revoked the old install's), copied over the copy's; a marker
//!   written into the copy. Then the swap: the new folder, if any, renamed to a free `<new>.first-start-backup[-N]`
//!   (never deleted, never replaced), and the copy renamed to `<new>`. The marker in place is the commit: that folder
//!   is carried, whatever the journal managed to say, so a retry never copies over what the app wrote since. A copy
//!   with its marker and nothing at `<new>` is a swap stopped between its two renames: only the second is left.
//! - **Provider keys** (`credentials::copy_from`): each old key the new namespace lacks is copied and read back;
//!   nothing is overwritten or deleted. The journal keeps each provider settled, so a key removed later is never
//!   copied again.
//! - **Finished** when the folders and every key are settled. A failure is logged, never fatal, and retried at the
//!   next launch. A reset finishes it before it clears anything (`stop`, from `data::begin_reset`).
//!
//! Not carried: the macOS logs, saved window state and `defaults` domain, the other caches, the macOS microphone grant
//! (the system keys it by bundle id) and the reset's control directory.

use crate::data::PAIRINGS_FILE;
use crate::platform::{account, app_data, credentials, reset_handoff};
use log::Level;
use serde::{Deserialize, Serialize};
use sidevoice_desktop_core::call_controls::PLACEMENTS_FILE;
use sidevoice_desktop_core::settings;
use sidevoice_local_host::store;
use sidevoice_platform::fs::{Create, Files, Fs};
use std::{
    cell::Cell,
    collections::BTreeSet,
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

/// Written into a copy before it is swapped in: the folder in place is the carried one.
const MARKER: &str = ".carried-from-dev.sidevoice.desktop";

/// The journal's content.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    /// Nothing more is done: everything is settled, or a reset ended it.
    finished: bool,
    /// Whether the install here was set up, decided once before anything is carried.
    configured: Option<bool>,
    /// Every folder is settled: carried, absent, or left because the install here was set up.
    folders: bool,
    /// The providers whose key is settled: copied, or the new namespace held one.
    keys: BTreeSet<String>,
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

/// A reset ends the carry-over, finished or not, before it clears anything: nothing it deletes may come back from the
/// old identifier's data. `control` is the reset's control directory.
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

    fn save(&self, control: &Path, state: &State) -> io::Result<()> {
        self.step()?;
        write_state(control, state)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.step()?;
        fs::rename(from, to)
    }

    fn copy(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.step()?;
        fs::copy(from, to).map(drop)
    }

    fn mark(&self, folder: &Path) -> io::Result<()> {
        self.step()?;
        fs::write(folder.join(MARKER), b"")
    }

    fn remove(&self, folder: &Path) -> io::Result<()> {
        self.step()?;
        fs::remove_dir_all(folder)
    }

    /// Copy the folder `from` to `to`, which does not exist: its folders and files, with their permissions. A link is
    /// neither followed nor copied (the app keeps none).
    fn copy_tree(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.step()?;
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let (from, to) = (entry.path(), to.join(entry.file_name()));
            let kind = entry.file_type()?;
            if kind.is_dir() {
                self.copy_tree(&from, &to)?;
            } else if kind.is_file() {
                self.copy(&from, &to)?;
            }
        }
        fs::set_permissions(to, fs::metadata(from)?.permissions())
    }
}

/// The log lines of a carry-over. The log does not exist yet when it runs (the carry-over must come first, before the
/// log plugin writes into the folders it carries), so they wait for [`Report::log`].
pub struct Report(Vec<(Level, String)>);

impl Report {
    /// Write the lines to the log, one per thing carried (or not).
    pub fn log(self) {
        for (level, line) in self.0 {
            log::log!(level, "legacy identifier: {line}");
        }
    }

    fn say(&mut self, level: Level, line: String) {
        self.0.push((level, line));
    }
}

/// Carry the old identifier's folders and provider keys over to the app's own. See the module.
pub fn carry_over() -> Report {
    let said = |line: &str| Report(vec![(Level::Info, line.to_owned())]);
    let Ok(control) = reset_handoff::state_dir() else { return said("no control directory: not looked at") };
    if !hold_instance(&control) {
        return said("another instance is running: left for a later launch");
    }
    match account::home().and_then(|home| app_data::carried_from(&home, LEGACY)) {
        Ok(folders) => run(&folders, &control, &Io::default(), |settled| credentials::copy_from(LEGACY, settled)),
        Err(key) => said(&format!("not looked at ({key})")),
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

/// [`carry_over`] for these `(old, new)` folders (the app data folder first), with the journal in `control`.
/// `copy_keys` copies the old provider keys but those it is given.
fn run(
    folders: &[(PathBuf, PathBuf)],
    control: &Path,
    io: &Io,
    copy_keys: impl FnOnce(BTreeSet<String>) -> Result<credentials::Copied, credentials::Error>,
) -> Report {
    let mut report = Report(Vec::new());
    let mut state = match read_state(control) {
        Ok(state) if state.finished => return report,
        Ok(state) => state,
        Err(error) => {
            report.say(Level::Warn, format!("the journal cannot be read ({}): nothing done", error.kind()));
            return report;
        }
    };
    if state.configured.is_none() {
        // Once, and written before anything is carried: carrying changes what this looks at.
        state.configured = Some(configured(&folders[0].1));
        if let Err(error) = io.save(control, &state) {
            report.say(Level::Warn, format!("the journal cannot be written ({}): nothing done", error.kind()));
            return report;
        }
    }
    if !state.folders {
        state.folders = true;
        for (from, to) in folders {
            state.folders &= folder(from, to, state.configured == Some(true), io, &mut report);
        }
    }
    let keys = keys(&mut state.keys, copy_keys, &mut report);
    state.finished = state.folders && keys;
    if let Err(error) = io.save(control, &state) {
        report.say(Level::Warn, format!("the journal cannot be written ({})", error.kind()));
    }
    report
}

/// One folder, reported if there was anything to carry; whether it is settled. Where the install was set up (`kept`),
/// an old folder that exists is only reported.
fn folder(from: &Path, to: &Path, kept: bool, io: &Io, report: &mut Report) -> bool {
    let (shown_from, shown_to) = (from.display(), to.display());
    if kept {
        if exists(from).unwrap_or(true) {
            report.say(Level::Info, format!("left {shown_from}: {shown_to} is set up"));
        }
        return true;
    }
    let line = match carry(from, to, io) {
        Ok(None) => return true,
        Ok(Some(None)) => format!("copied {shown_from} to {shown_to}"),
        Ok(Some(Some(backup))) => {
            format!("copied {shown_from} to {shown_to}; what was there is kept as {}", backup.display())
        }
        Err(error) => {
            report.say(Level::Warn, format!("could not copy {shown_from} to {shown_to}: {}", error.kind()));
            return false;
        }
    };
    report.say(Level::Info, line);
    true
}

/// Carry the old folder `from` to `to` (see the module). `None` when there is nothing to carry: no old folder, or one
/// carried already. Else where the folder that was at `to` is kept, if there was one.
fn carry(from: &Path, to: &Path, io: &Io) -> io::Result<Option<Option<PathBuf>>> {
    if !exists(from)? || exists(&to.join(MARKER))? {
        return Ok(None);
    }
    let copy = sibling(to, ".migrating");
    if exists(to)? || !exists(&copy.join(MARKER))? {
        if exists(&copy)? {
            io.remove(&copy)?;
        }
        let made = io.copy_tree(from, &copy).and_then(|()| {
            let live = to.join(store::FILE_NAME);
            if exists(&live)? {
                io.copy(&live, &copy.join(store::FILE_NAME))?;
            }
            io.mark(&copy)
        });
        if made.is_err() {
            // Not a step: a copy that did not go in takes space for nothing.
            let _ = fs::remove_dir_all(&copy);
        }
        made?;
    }
    let backup = if exists(to)? { Some(backup_of(to)?) } else { None };
    if let Some(backup) = &backup {
        io.rename(to, backup)?;
    }
    io.rename(&copy, to)?;
    Ok(Some(backup))
}

/// A free name next to `to` for the folder a carried one takes the place of: `<to>.first-start-backup`, else the first
/// free `-2`, `-3`… An earlier backup is never replaced.
fn backup_of(to: &Path) -> io::Result<PathBuf> {
    let mut backup = sibling(to, ".first-start-backup");
    for n in 2.. {
        if !exists(&backup)? {
            break;
        }
        backup = sibling(to, &format!(".first-start-backup-{n}"));
    }
    Ok(backup)
}

/// `path` with `suffix` added to its name.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// The provider keys not yet `settled`, which gains those that are; whether they all are.
fn keys(
    settled: &mut BTreeSet<String>,
    copy_keys: impl FnOnce(BTreeSet<String>) -> Result<credentials::Copied, credentials::Error>,
    report: &mut Report,
) -> bool {
    let copied = match copy_keys(settled.clone()) {
        Ok(copied) => copied,
        Err(error) => {
            report.say(Level::Warn, format!("provider keys not looked at ({error:?})"));
            return false;
        }
    };
    let mut all = true;
    for (provider, result) in copied {
        match result {
            Ok(true) => report.say(Level::Info, format!("copied the {provider} key to the new namespace")),
            Ok(false) => report.say(Level::Info, format!("left the old {provider} key: the new namespace holds one")),
            Err(error) => {
                report.say(Level::Warn, format!("could not copy the {provider} key: {error:?}"));
                all = false;
                continue;
            }
        }
        settled.insert(provider);
    }
    all
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
    use credentials::{Copied, Error};
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

    fn no_keys(_: BTreeSet<String>) -> Result<Copied, Error> {
        Ok(Vec::new())
    }

    type Folders = Vec<(PathBuf, PathBuf)>;

    /// The old install and the first start of the new one: what the nightly of #316 left.
    fn nightly(root: &Path) -> Folders {
        let folders =
            vec![(root.join("old-data"), root.join("new-data")), (root.join("old-web"), root.join("new-web"))];
        let ((old_data, new_data), (old_web, new_web)) = (&folders[0], &folders[1]);
        put(old_data, "note", "old settings");
        put(old_data, settings::FILE_NAME, "{}");
        put(&old_data.join("nested"), "deep", "old deep");
        put(old_data, store::FILE_NAME, "revoked token");
        put(old_web, "note", "old web");
        put(&new_data.join("logs"), "log", "launch");
        put(new_data, store::FILE_NAME, "live token");
        put(new_web, "note", "first-start web");
        folders
    }

    fn healthy(folders: &Folders, control: &Path) -> Report {
        run(folders, control, &Io::default(), no_keys)
    }

    /// The old install is exactly as it was: nothing of it is ever moved, changed or deleted.
    fn assert_old_intact(folders: &Folders) {
        assert_eq!(read(&folders[0].0, "note"), "old settings");
        assert_eq!(read(&folders[0].0.join("nested"), "deep"), "old deep");
        assert_eq!(read(&folders[0].0, store::FILE_NAME), "revoked token");
        assert!(!folders[0].0.join(MARKER).exists());
        assert_eq!(read(&folders[1].0, "note"), "old web");
    }

    /// The old install is in place with `token` as the local host's pairing, nothing is left over, and it is finished.
    fn assert_carried(folders: &Folders, control: &Path, token: &str) {
        assert_old_intact(folders);
        let ((_, new_data), (_, new_web)) = (&folders[0], &folders[1]);
        assert_eq!(read(new_data, "note"), "old settings");
        assert_eq!(read(&new_data.join("nested"), "deep"), "old deep");
        assert_eq!(read(new_data, store::FILE_NAME), token);
        assert_eq!(read(new_web, "note"), "old web");
        assert!(!sibling(new_data, ".migrating").exists() && !sibling(new_web, ".migrating").exists());
        assert!(read_state(control).unwrap().finished);
    }

    /// Whether `text` is in `name` of `dir` or of one of its backups.
    fn kept(dir: &Path, name: &str, text: &str) -> bool {
        let backups = (1..5).map(|n| match n {
            1 => sibling(dir, ".first-start-backup"),
            n => sibling(dir, &format!(".first-start-backup-{n}")),
        });
        std::iter::once(dir.to_path_buf())
            .chain(backups)
            .any(|dir| fs::read_to_string(dir.join(name)).is_ok_and(|found| found == text))
    }

    #[test]
    fn a_fresh_new_install_next_to_old_data_is_replaced_by_a_copy_and_kept_beside_it() {
        let (_temp, root) = scratch();
        let (folders, control) = (nightly(&root), root.join("control"));

        let report = healthy(&folders, &control);

        assert_carried(&folders, &control, "live token");
        assert_eq!(read(&root.join("new-data.first-start-backup"), store::FILE_NAME), "live token");
        assert!(root.join("new-data.first-start-backup/logs/log").exists());
        assert_eq!(read(&root.join("new-web.first-start-backup"), "note"), "first-start web");
        assert_eq!(report.0.len(), 2, "one line per folder");
        assert!(report.0.iter().all(|(level, line)| *level == Level::Info && line.contains("first-start-backup")));

        // Finished: an old folder that changes later is not carried again.
        put(&folders[1].0, "note", "another old web");
        assert!(healthy(&folders, &control).0.is_empty());
        assert_eq!(read(&folders[1].1, "note"), "old web");
    }

    #[test]
    fn a_set_up_install_here_is_left_as_it_is_and_so_is_the_old_one() {
        let (_temp, root) = scratch();
        let (folders, control) = (nightly(&root), root.join("control"));
        put(&folders[0].1, PAIRINGS_FILE, "{}");

        let report = healthy(&folders, &control);

        assert_old_intact(&folders);
        assert!(!folders[0].1.join("note").exists());
        assert_eq!(read(&folders[1].1, "note"), "first-start web");
        assert!(!root.join("new-data.first-start-backup").exists() && !root.join("new-data.migrating").exists());
        assert!(read_state(&control).unwrap().finished);
        assert_eq!(report.0.len(), 2);
        assert!(report.0.iter().all(|(level, line)| *level == Level::Info && line.starts_with("left ")));
    }

    #[test]
    fn an_old_install_is_copied_where_nothing_has_run_and_a_fresh_install_just_finishes() {
        let (_temp, root) = scratch();
        let control = root.join("control");
        let folders = vec![(root.join("old-data"), root.join("new-data"))];
        put(&folders[0].0, "note", "old settings");
        // A folder's permissions come with it (a private browser store stays private).
        let (old, new) = (folders[0].0.join("store"), folders[0].1.join("store"));
        put(&old, "data", "old store");
        let writable = fs::metadata(&old).unwrap().permissions();
        let mut read_only = writable.clone();
        read_only.set_readonly(true);
        fs::set_permissions(&old, read_only).unwrap();

        healthy(&folders, &control);

        assert_eq!(read(&folders[0].1, "note"), "old settings");
        assert_eq!(read(&new, "data"), "old store");
        assert!(fs::metadata(&new).unwrap().permissions().readonly());
        assert_eq!(read(&folders[0].0, "note"), "old settings", "the old one stays");
        assert!(!root.join("new-data.first-start-backup").exists());
        assert!(read_state(&control).unwrap().finished);
        for dir in [&old, &new] {
            fs::set_permissions(dir, writable.clone()).unwrap();
        }

        let (_fresh, root) = scratch();
        let folders = vec![(root.join("old-data"), root.join("new-data"))];
        assert!(healthy(&folders, &root.join("control")).0.is_empty());
        assert!(!folders[0].1.exists(), "nothing is created");
        assert!(read_state(&root.join("control")).unwrap().finished);
    }

    #[test]
    fn every_crash_point_loses_nothing_and_the_next_launch_finishes() {
        for crash_at in 0.. {
            let (_temp, root) = scratch();
            let (folders, control) = (nightly(&root), root.join("control"));
            let io = Io { crash_at: Some(crash_at), ..Io::default() };
            run(&folders, &control, &io, no_keys);
            let crashed = io.calls.get() > crash_at;

            // Wherever it stopped: the old install as it was, and what the first start made in place or kept.
            assert_old_intact(&folders);
            assert!(kept(&folders[0].1, store::FILE_NAME, "live token"), "crash at {crash_at}");
            assert!(kept(&folders[1].1, "note", "first-start web"), "crash at {crash_at}");

            // The next launch comes before the app runs: it finishes, with the live token, and one backup each.
            healthy(&folders, &control);
            assert_carried(&folders, &control, "live token");
            assert_eq!(read(&root.join("new-data.first-start-backup"), store::FILE_NAME), "live token");
            assert_eq!(read(&root.join("new-web.first-start-backup"), "note"), "first-start web");
            assert!(!root.join("new-data.first-start-backup-2").exists(), "crash at {crash_at}");
            if !crashed {
                break;
            }
        }
    }

    #[test]
    fn every_single_failure_is_retried_with_the_first_decision_and_never_overwrites_what_the_app_wrote_since() {
        for fail_at in 0.. {
            let (_temp, root) = scratch();
            let (folders, control) = (nightly(&root), root.join("control"));
            let io = Io { fail_at: Some(fail_at), ..Io::default() };
            run(&folders, &control, &io, no_keys);
            let failed = io.calls.get() > fail_at;
            assert_old_intact(&folders);

            // The app runs between the launches, on whatever is there: it pairs again with the local core, and, once
            // the decision is written, the person saves a setting (a new decision would say "set up").
            let (new_data, new_web) = (&folders[0].1, &folders[1].1);
            put(new_data, store::FILE_NAME, "token after");
            let decided = read_state(&control).unwrap().configured == Some(false);
            if decided {
                put(new_data, settings::FILE_NAME, "between");
            }
            if !new_web.exists() {
                put(new_web, "note", "first start");
            }

            healthy(&folders, &control);
            assert_carried(&folders, &control, "token after");
            assert!(!decided || kept(new_data, settings::FILE_NAME, "between"), "failure at {fail_at}");
            if !failed {
                break;
            }
        }
    }

    #[test]
    fn an_earlier_backup_is_never_replaced_and_a_leftover_copy_is() {
        let (_temp, root) = scratch();
        let (folders, control) = (nightly(&root), root.join("control"));
        put(&root.join("new-data.first-start-backup"), "note", "earlier");
        put(&root.join("new-data.migrating"), "stray", "half a copy");

        healthy(&folders, &control);

        assert_carried(&folders, &control, "live token");
        assert_eq!(read(&root.join("new-data.first-start-backup"), "note"), "earlier");
        assert_eq!(read(&root.join("new-data.first-start-backup-2"), store::FILE_NAME), "live token");
        assert!(!folders[0].1.join("stray").exists());
    }

    #[test]
    fn a_reset_before_the_carry_over_during_it_or_after_it_brings_nothing_back() {
        // A reset (`stop`, then the deletion of the app's folders: not the backups or a leftover copy).
        let reset = |folders: &Folders, control: &Path| {
            stop(control).unwrap();
            for (_, new) in folders {
                let _ = fs::remove_dir_all(new);
            }
        };
        let after_it = |folders: &Folders, control: &Path, at: usize| {
            let report = run(folders, control, &Io::default(), |_| unreachable!("no key after a reset"));
            assert!(report.0.is_empty(), "at {at}");
            assert!(folders.iter().all(|(_, new)| !new.exists()), "at {at}");
            assert_old_intact(folders);
        };

        // Before.
        let (_temp, root) = scratch();
        let (folders, control) = (nightly(&root), root.join("control"));
        reset(&folders, &control);
        after_it(&folders, &control, 0);

        // During: a launch stopped at any point, then a reset.
        for crash_at in 0..20 {
            let (_temp, root) = scratch();
            let (folders, control) = (nightly(&root), root.join("control"));
            run(&folders, &control, &Io { crash_at: Some(crash_at), ..Io::default() }, no_keys);
            reset(&folders, &control);
            after_it(&folders, &control, crash_at);
        }

        // After.
        let (_temp, root) = scratch();
        let (folders, control) = (nightly(&root), root.join("control"));
        healthy(&folders, &control);
        reset(&folders, &control);
        after_it(&folders, &control, 0);
    }

    #[test]
    fn an_unavailable_key_store_is_asked_again_and_the_folders_are_not() {
        let (_temp, root) = scratch();
        let (folders, control) = (nightly(&root), root.join("control"));

        let report = run(&folders, &control, &Io::default(), |_| Err(Error::Unavailable));
        assert_carried_but_open(&folders, &control);
        assert!(report.0.iter().any(|(level, line)| *level == Level::Warn && line.contains("Unavailable")));

        // Deleted by hand meanwhile: the folders are settled, so it does not come back.
        fs::remove_dir_all(&folders[1].1).unwrap();
        run(&folders, &control, &Io::default(), |_| Ok(vec![("openai".into(), Ok(true))]));
        assert!(read_state(&control).unwrap().finished);
        assert!(!folders[1].1.exists());
    }

    fn assert_carried_but_open(folders: &Folders, control: &Path) {
        assert_eq!(read(&folders[0].1, "note"), "old settings");
        let state = read_state(control).unwrap();
        assert!(state.folders && !state.finished);
    }

    #[test]
    fn a_settled_key_is_never_copied_again_and_an_unsettled_one_is_retried() {
        let (_temp, root) = scratch();
        let (folders, control) = (nightly(&root), root.join("control"));
        run(&folders, &control, &Io::default(), |settled| {
            assert!(settled.is_empty());
            Ok(vec![("openai".into(), Ok(true)), ("mistral".into(), Ok(false)), ("other".into(), Err(Error::Failed))])
        });
        assert_carried_but_open(&folders, &control);

        // The person removes the openai key meanwhile: it is settled, and not copied again.
        let report = run(&folders, &control, &Io::default(), |settled| {
            assert_eq!(settled, BTreeSet::from(["mistral".to_owned(), "openai".to_owned()]));
            Ok(vec![("other".into(), Ok(true))])
        });
        assert!(read_state(&control).unwrap().finished);
        assert_eq!(report.0.len(), 1);
    }

    #[test]
    fn a_journal_that_cannot_be_read_does_nothing() {
        let (_temp, root) = scratch();
        let (folders, control) = (nightly(&root), root.join("control"));
        fs::write(control.join(JOURNAL), "{ not json").unwrap();
        let report = run(&folders, &control, &Io::default(), |_| unreachable!("nothing done"));
        assert_eq!(report.0.len(), 1);
        assert_eq!(read(&folders[0].1, store::FILE_NAME), "live token");
        assert!(!folders[0].1.join("note").exists());
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
    fn a_failure_does_not_stop_the_next_folder() {
        let (_temp, root) = scratch();
        let control = root.join("control");
        // The second folder cannot be copied: its destination is below a file.
        fs::write(root.join("blocker"), "a file").unwrap();
        let folders = vec![
            (root.join("old-data"), root.join("new-data")),
            (root.join("old-failing"), root.join("blocker/new-failing")),
            (root.join("old-web"), root.join("new-web")),
        ];
        for (old, _) in &folders {
            put(old, "note", "old");
        }

        let report = healthy(&folders, &control);

        assert_eq!(read(&folders[0].1, "note"), "old");
        assert_eq!(read(&folders[2].1, "note"), "old");
        let levels: Vec<Level> = report.0.iter().map(|(level, _)| *level).collect();
        assert_eq!(levels, vec![Level::Info, Level::Warn, Level::Info]);
        let state = read_state(&control).unwrap();
        assert!(!state.folders && !state.finished);
    }
}
