//! One-off carry-over of an install's data from the bundle identifier the app had before `ai.sidevoice.desktop`
//! (`dev.sidevoice.desktop`), so a person who updates keeps their settings, pairings, browser storage and provider keys.
//! It also serves the builds that already ran under the new identifier with no carry-over (the nightly of #316): their
//! folders are first-start leftovers, and the old install's data is still where it was.
//!
//! KEEP THIS MODULE UNTIL 1.0 (RELEASING.md, "Legacy identifier carry-over"); removing it needs a release note that
//! says "update through vX.Y first". What exists only for it: `platform::app_data::{carried_from, create_folder}`,
//! `platform::webview_data::locations_of`, `platform::credentials::{providers_in, copy_key_from}`,
//! `data::PAIRINGS_FILE` unless something else uses it, and the `stop` call in `data::begin_reset`.
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
//! new identifier from before this module (the nightly of #316) takes no lock and is not detected: the updater quits
//! the app before it installs (docs/APP_DATA.md). A running build of the old identifier is not detected either (its
//! single-instance lock is the plugin's private socket, mutex or bus name on each OS): nothing of it changes, and a copy
//! made while it wrote is thrown away (below).
//!
//! - **Decided once**, before anything is carried, and written to the journal: whether the install here was set up
//!   (any of the settings, the card's placements or the remote pairings in its app data folder). If it was, no folder
//!   is touched.
//! - **Folders** (the app data and the browser storage, `app_data::carried_from`), each old one that exists, through a
//!   copy of this module's own beside the new one: `<new>.migrating-<random>`, made with `create_dir` (never one that
//!   was there) and private until complete, an owner file written into it first. Only a folder so named with that
//!   file in it (or empty) is ever removed, through the shared held removal (a link is removed, never followed; a
//!   mount below is refused). Into it: the old folder (each folder private from its creation and given its own
//!   permissions once its contents are in), compared before and after (every entry's size and modification time: a
//!   running old build may write to it), else thrown away and tried at the next launch; the new folder's
//!   `local-host.json`, the live token of the local host (pairing the first start revoked the old install's); a marker.
//!   Then the swap: the new folder, if any, renamed to a free `<new>.first-start-backup[-N]` (never deleted, never
//!   replaced), and the copy renamed to `<new>`. The marker in place is the commit: that folder is carried, whatever
//!   the journal managed to say, so a retry never copies over what the app wrote since. A copy with its marker and
//!   nothing at `<new>` is a swap stopped between its two renames: only the second is left.
//! - **Provider keys** (`credentials::copy_key_from`): each old key the new namespace lacks is copied and read back,
//!   one at a time, each with its own admission and deadline; nothing is overwritten or deleted. Each provider is
//!   settled by a record beside the journal (`legacy-identifier.key-<provider>`), written by the store's worker before
//!   it writes, even after the launch stopped waiting for it, and refused once a reset ended the carry-over: a key is
//!   never copied twice, so one removed later never comes back.
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
use sidevoice_platform::fs::{Access, Create, Directory, Files, Fs, HeldDirectory};
use sidevoice_platform::removal::Removal;
use std::{
    cell::Cell,
    fs, io,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::SystemTime,
};

/// The bundle identifier of the builds before `ai.sidevoice.desktop`.
const LEGACY: &str = "dev.sidevoice.desktop";

/// The journal, in the reset's control directory.
const JOURNAL: &str = "legacy-identifier.json";

/// The record of a settled provider key, beside the journal: this, then the provider id.
const KEY_RECORD: &str = "legacy-identifier.key-";

/// The lock a running instance holds, in the same directory.
const INSTANCE_LOCK: &str = "instance.lock";

/// What a copy of this module's own is named: the new folder's name, this, then a random number.
const COPY: &str = ".migrating-";

/// Written first into a copy of this module's own: it is this module's to remove.
const OWNER: &str = ".carry-over-copy";

/// Written last into a copy, before it is swapped in: the folder in place is the carried one.
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
/// one after it as a crash does (`crash_at`), or do something of its own before each (`meddle`).
#[derive(Default)]
struct Io {
    calls: Cell<usize>,
    fail_at: Option<usize>,
    crash_at: Option<usize>,
    meddle: Option<Box<dyn Fn(usize)>>,
}

impl Io {
    fn step(&self) -> io::Result<()> {
        let call = self.calls.get();
        self.calls.set(call + 1);
        if let Some(meddle) = &self.meddle {
            meddle(call);
        }
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

    /// A file copied with its permissions: `fs::copy` creates it with the source's mode (Unix; a clone on APFS), so it
    /// is never more open than the source; on Windows it inherits the folder's ACL.
    fn copy(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.step()?;
        fs::copy(from, to).map(drop)
    }

    fn create_folder(&self, path: &Path) -> io::Result<()> {
        self.step()?;
        app_data::create_folder(path)
    }

    fn touch(&self, path: &Path) -> io::Result<()> {
        self.step()?;
        fs::write(path, b"")
    }

    fn remove(&self, copy: &Path) -> io::Result<()> {
        self.step()?;
        remove_copy(copy)
    }

    /// Copy what the folder `from` holds into the folder `to`: each folder private from its creation and given its own
    /// permissions once its contents are in, each file with its own. A link is neither followed nor copied (the app
    /// keeps none).
    fn copy_into(&self, from: &Path, to: &Path) -> io::Result<()> {
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let (from, to) = (entry.path(), to.join(entry.file_name()));
            let kind = entry.file_type()?;
            if kind.is_dir() {
                self.create_folder(&to)?;
                self.copy_into(&from, &to)?;
                fs::set_permissions(&to, fs::metadata(&from)?.permissions())?;
            } else if kind.is_file() {
                self.copy(&from, &to)?;
            }
        }
        Ok(())
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

/// What settles one provider's key: run by the store's worker before it writes (`credentials::copy_key_from`).
type Settle = Box<dyn FnOnce() -> bool + Send>;

/// Carry the old identifier's folders and provider keys over to the app's own. See the module.
pub fn carry_over() -> Report {
    let said = |line: &str| Report(vec![(Level::Info, line.to_owned())]);
    let Ok(control) = reset_handoff::state_dir() else { return said("no control directory: not looked at") };
    if !hold_instance(&control) {
        return said("another instance is running: left for a later launch");
    }
    match account::home().and_then(|home| app_data::carried_from(&home, LEGACY)) {
        Ok(folders) => run(
            &folders,
            &control,
            &Io::default(),
            || credentials::providers_in(LEGACY),
            |provider, settle| credentials::copy_key_from(LEGACY, provider, settle),
        ),
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

/// [`carry_over`] for these `(old, new)` folders (the app data folder first), with the journal in `control`. `list`
/// names the providers with an old key; `copy` copies one, settling it as [`Settle`] says.
fn run(
    folders: &[(PathBuf, PathBuf)],
    control: &Path,
    io: &Io,
    list: impl FnOnce() -> Result<Vec<String>, credentials::Error>,
    copy: impl FnMut(&str, Settle) -> Result<bool, credentials::Error>,
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
    let keys = keys(control, list, copy, &mut report);
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
    let mut copies = copies_of(to)?;
    let ready = match exists(to)? {
        false => copies.iter().position(|copy| copy.join(MARKER).exists()).map(|at| copies.swap_remove(at)),
        true => None,
    };
    for leftover in &copies {
        // One that cannot be removed only takes space.
        let _ = io.remove(leftover);
    }
    let copy = match ready {
        Some(copy) => copy,
        None => make_copy(from, to, io)?,
    };
    let backup = if exists(to)? { Some(backup_of(to)?) } else { None };
    if let Some(backup) = &backup {
        io.rename(to, backup)?;
    }
    io.rename(&copy, to)?;
    Ok(Some(backup))
}

/// A complete copy of `from` beside `to`, of this module's own (see the module). One that fails, or that finds `from`
/// changed while it was copied, is removed.
fn make_copy(from: &Path, to: &Path, io: &Io) -> io::Result<PathBuf> {
    let mut random = [0; 8];
    getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
    let copy = sibling(to, &format!("{COPY}{:016x}", u64::from_le_bytes(random)));
    io.create_folder(&copy)?;
    let made = io.touch(&copy.join(OWNER)).and_then(|()| {
        let before = snapshot(from)?;
        io.copy_into(from, &copy)?;
        if snapshot(from)? != before {
            return Err(io::Error::new(io::ErrorKind::ResourceBusy, "the old folder changed while it was copied"));
        }
        // The local host's pairing outlives a first start: the core revoked the old install's token when it paired.
        let live = to.join(store::FILE_NAME);
        if exists(&live)? {
            io.copy(&live, &copy.join(store::FILE_NAME))?;
        }
        io.touch(&copy.join(MARKER))?;
        fs::set_permissions(&copy, fs::metadata(from)?.permissions())
    });
    match made {
        Ok(()) => Ok(copy),
        Err(error) => {
            // Not a step: a copy that did not go in only takes space.
            let _ = remove_copy(&copy);
            Err(error)
        }
    }
}

/// The copies of this module's own beside `to`: folders named `<to>.migrating-…` with the owner file in them, or empty
/// (stopped before it was written: removing one loses nothing). Any other folder there is someone else's.
fn copies_of(to: &Path) -> io::Result<Vec<PathBuf>> {
    let (Some(parent), Some(name)) = (to.parent(), to.file_name()) else { return Ok(Vec::new()) };
    let prefix = format!("{}{COPY}", name.to_string_lossy());
    let mut copies = Vec::new();
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with(&prefix)
            && entry.file_type()?.is_dir()
            && (fs::symlink_metadata(path.join(OWNER)).is_ok_and(|owner| owner.is_file())
                || fs::read_dir(&path)?.next().is_none())
        {
            copies.push(path);
        }
    }
    Ok(copies)
}

/// Remove a copy of this module's own through the shared held removal: a link in it is removed and never followed, and
/// a mount below it, or a folder others may write, is refused (`std::fs::remove_dir_all` would descend into a mount on
/// Unix).
fn remove_copy(copy: &Path) -> io::Result<()> {
    let (Some(parent), Some(name)) = (copy.parent(), copy.file_name()) else {
        return Err(io::ErrorKind::InvalidInput.into());
    };
    HeldDirectory::open(parent, Access::NotWritableByOthers)?.remove_contents(&[name])
}

/// Every entry under `dir` (links not followed) with its size and modification time, in order: unchanged across a
/// copy, nothing wrote to it meanwhile.
fn snapshot(dir: &Path) -> io::Result<Vec<(PathBuf, u64, Option<SystemTime>)>> {
    let (mut entries, mut folders) = (Vec::new(), vec![dir.to_path_buf()]);
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder)? {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                folders.push(path.clone());
            }
            entries.push((path, metadata.len(), metadata.modified().ok()));
        }
    }
    entries.sort();
    Ok(entries)
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

/// The provider keys: each listed provider not yet settled is copied by `copy`, with a [`Settle`] that writes its
/// record; whether all are settled.
fn keys(
    control: &Path,
    list: impl FnOnce() -> Result<Vec<String>, credentials::Error>,
    mut copy: impl FnMut(&str, Settle) -> Result<bool, credentials::Error>,
    report: &mut Report,
) -> bool {
    let providers = match list() {
        Ok(providers) => providers,
        Err(error) => {
            report.say(Level::Warn, format!("provider keys not looked at ({error:?})"));
            return false;
        }
    };
    let mut all = true;
    for provider in providers {
        let record = control.join(format!("{KEY_RECORD}{provider}"));
        match exists(&record) {
            Ok(false) => (),
            Ok(true) => continue,
            Err(_) => {
                all = false;
                continue;
            }
        }
        let control = control.to_owned();
        match copy(&provider, Box::new(move || settle(&control, &record))) {
            Ok(true) => report.say(Level::Info, format!("copied the {provider} key to the new namespace")),
            Ok(false) => {
                report.say(Level::Info, format!("nothing to copy for {provider}: the new namespace holds a key"))
            }
            Err(error) => {
                report.say(Level::Warn, format!("could not copy the {provider} key: {error:?}"));
                all = false;
            }
        }
    }
    all
}

/// Settle a provider's key by writing its `record` in `control`, unless a reset ended the carry-over: then nothing is
/// written, and no key is copied after a reset. Whether it is settled.
fn settle(control: &Path, record: &Path) -> bool {
    matches!(read_state(control), Ok(state) if !state.finished) && Files::write_private(record, b"").is_ok()
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
    use std::rc::Rc;

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

    fn no_keys() -> Result<Vec<String>, Error> {
        Ok(Vec::new())
    }

    fn no_copy(_: &str, _: Settle) -> Result<bool, Error> {
        unreachable!("no key to copy")
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
        run(folders, control, &Io::default(), no_keys, no_copy)
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
        assert!(copies_of(new_data).unwrap().is_empty() && copies_of(new_web).unwrap().is_empty(), "no copy left");
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
            run(&folders, &control, &io, no_keys, no_copy);
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
            run(&folders, &control, &io, no_keys, no_copy);
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
    fn an_earlier_backup_and_folders_not_ours_are_never_touched_and_our_own_leftovers_go() {
        let (_temp, root) = scratch();
        let (folders, control) = (nightly(&root), root.join("control"));
        put(&root.join("new-data.first-start-backup"), "note", "earlier");
        // Someone else's: the name this module used to give its copy, and its pattern without the owner file.
        let theirs = [root.join("new-data.migrating"), root.join("new-data.migrating-0123456789abcdef")];
        for dir in &theirs {
            put(dir, "mine", "a person's file");
        }
        // This module's own: an interrupted copy with a link out of it, and one stopped before its owner file.
        let (ours, outside) = (root.join("new-data.migrating-feedfacefeedface"), root.join("outside"));
        put(&ours, OWNER, "");
        put(&outside, "kept", "outside the copy");
        Files::symlink(&outside, ours.join("link"));
        let empty = root.join("new-web.migrating-00000000deadbeef");
        fs::create_dir(&empty).unwrap();
        let unrelated = root.join("new-web.other");
        fs::create_dir(&unrelated).unwrap();

        healthy(&folders, &control);

        assert_carried(&folders, &control, "live token");
        assert_eq!(read(&root.join("new-data.first-start-backup"), "note"), "earlier");
        assert_eq!(read(&root.join("new-data.first-start-backup-2"), store::FILE_NAME), "live token");
        assert!(theirs.iter().all(|dir| read(dir, "mine") == "a person's file"));
        assert!(!ours.exists() && !empty.exists() && unrelated.exists());
        assert_eq!(read(&outside, "kept"), "outside the copy", "a link is removed, never followed");
    }

    #[test]
    fn a_copy_lets_no_one_else_in_until_it_is_complete() {
        let (_temp, root) = scratch();
        let (folders, control) = (nightly(&root), root.join("control"));
        let private = fs::metadata(&control).unwrap().permissions();
        let checked = Rc::new(Cell::new(0));
        let (seen, scratch_root) = (checked.clone(), root.clone());
        // Before every call: a copy without its marker, and every folder in it still empty, is private.
        let meddle = move |_| {
            for copy in fs::read_dir(&scratch_root).unwrap().map(|entry| entry.unwrap().path()) {
                if !copy.to_string_lossy().contains(COPY) || copy.join(MARKER).exists() {
                    continue;
                }
                assert_eq!(fs::metadata(&copy).unwrap().permissions(), private, "{}", copy.display());
                let mut folders = vec![copy];
                while let Some(folder) = folders.pop() {
                    let inside: Vec<_> = fs::read_dir(&folder).unwrap().map(|entry| entry.unwrap().path()).collect();
                    if inside.is_empty() {
                        assert_eq!(fs::metadata(&folder).unwrap().permissions(), private, "{}", folder.display());
                    }
                    folders.extend(inside.into_iter().filter(|path| path.is_dir()));
                    seen.set(seen.get() + 1);
                }
            }
        };

        run(&folders, &control, &Io { meddle: Some(Box::new(meddle)), ..Io::default() }, no_keys, no_copy);

        assert_carried(&folders, &control, "live token");
        assert!(checked.get() > 0);
    }

    #[test]
    fn an_old_folder_that_changes_while_it_is_copied_is_not_carried_and_is_tried_again() {
        let (_temp, root) = scratch();
        let (folders, control) = (nightly(&root), root.join("control"));
        // A running old build writes to it in the middle of the app data's copy (after its folder and owner file).
        let note = folders[0].0.join("note");
        let meddle = move |call| {
            if call == 4 {
                fs::write(&note, "written while copied").unwrap();
            }
        };

        let report = run(&folders, &control, &Io { meddle: Some(Box::new(meddle)), ..Io::default() }, no_keys, no_copy);

        let new_data = &folders[0].1;
        assert!(!new_data.join("note").exists() && !new_data.join(MARKER).exists());
        assert_eq!(read(new_data, store::FILE_NAME), "live token");
        assert!(copies_of(new_data).unwrap().is_empty(), "the copy is thrown away");
        assert_eq!(read(&folders[1].1, "note"), "old web", "the other folder goes on");
        assert!(report.0.iter().any(|(level, line)| *level == Level::Warn && line.contains("resource busy")));
        assert!(!read_state(&control).unwrap().finished);

        healthy(&folders, &control);
        assert_eq!(read(new_data, "note"), "written while copied");
        assert!(read_state(&control).unwrap().finished);
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
            let report = run(folders, control, &Io::default(), || unreachable!("no key after a reset"), no_copy);
            assert!(report.0.is_empty(), "at {at}");
            assert!(folders.iter().all(|(_, new)| !new.exists()), "at {at}");
            assert_old_intact(folders);
            // A key copy that completes after the reset is refused, and records nothing.
            let record = control.join(format!("{KEY_RECORD}openai"));
            assert!(!settle(control, &record) && !record.exists(), "at {at}");
        };

        // Before.
        let (_temp, root) = scratch();
        let (folders, control) = (nightly(&root), root.join("control"));
        reset(&folders, &control);
        after_it(&folders, &control, 0);

        // During: a launch stopped at any point, then a reset.
        for crash_at in 0..24 {
            let (_temp, root) = scratch();
            let (folders, control) = (nightly(&root), root.join("control"));
            run(&folders, &control, &Io { crash_at: Some(crash_at), ..Io::default() }, no_keys, no_copy);
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

        let report = run(&folders, &control, &Io::default(), || Err(Error::Unavailable), no_copy);
        assert_carried_but_open(&folders, &control);
        assert!(report.0.iter().any(|(level, line)| *level == Level::Warn && line.contains("Unavailable")));

        // Deleted by hand meanwhile: the folders are settled, so it does not come back.
        fs::remove_dir_all(&folders[1].1).unwrap();
        let copied = |_: &str, settle: Settle| Ok(settle());
        run(&folders, &control, &Io::default(), || Ok(vec!["openai".into()]), copied);
        assert!(read_state(&control).unwrap().finished);
        assert!(!folders[1].1.exists());
    }

    fn assert_carried_but_open(folders: &Folders, control: &Path) {
        assert_eq!(read(&folders[0].1, "note"), "old settings");
        let state = read_state(control).unwrap();
        assert!(state.folders && !state.finished);
    }

    #[test]
    fn a_key_copy_that_times_out_is_settled_when_it_completes_and_never_copied_again() {
        let (_temp, root) = scratch();
        let (folders, control) = (nightly(&root), root.join("control"));
        let list = || Ok(vec!["first".into(), "second".into(), "third".into()]);
        let late = std::cell::RefCell::new(None);
        // The first copies; the second's prompt outlasts its deadline and completes later on the store's worker; the
        // third finds that worker busy and does not run.
        let copy = |provider: &str, settle: Settle| match provider {
            "first" => Ok(settle()),
            "second" => {
                *late.borrow_mut() = Some(std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    settle()
                }));
                Err(Error::Unavailable)
            }
            _ => Err(Error::Unavailable),
        };
        run(&folders, &control, &Io::default(), list, copy);
        assert_carried_but_open(&folders, &control);
        assert!(late.take().unwrap().join().unwrap(), "settled after the launch stopped waiting");

        // The next launch: only the third is asked for.
        let asked = std::cell::RefCell::new(Vec::new());
        let copy = |provider: &str, settle: Settle| {
            asked.borrow_mut().push(provider.to_owned());
            Ok(settle())
        };
        run(&folders, &control, &Io::default(), list, copy);
        assert_eq!(asked.take(), ["third"]);
        assert!(read_state(&control).unwrap().finished);
    }

    #[test]
    fn a_journal_that_cannot_be_read_does_nothing() {
        let (_temp, root) = scratch();
        let (folders, control) = (nightly(&root), root.join("control"));
        fs::write(control.join(JOURNAL), "{ not json").unwrap();
        let report = run(&folders, &control, &Io::default(), || unreachable!("nothing done"), no_copy);
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
