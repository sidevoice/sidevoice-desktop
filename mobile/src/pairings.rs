//! The pairings the room page keeps (the machines it is paired with and the revocations still to tell them), as one JSON
//! file, alone in a folder of its own in the app's private storage (`pairings/pairings.json`): read and written whole,
//! as the page's JSON, with the refusals the page already knows from the desktop (`data.pairings-invalid`, `-read`,
//! `-write`; docs/BRIDGE.md). `read` and `write` are the pair of the shared plugin's `PairingStore`, which the app
//! implements for this file when the plugin joins the app (row 49: the mobile crate does not depend on
//! `sidevoice-app-tauri` before its engine and voice build for the phone). `PairingFile::of_app(app)` is the constructor
//! that picks the file.
//!
//! A write is atomic: the new record is written to a temporary file in the same directory, synced, renamed over the old
//! one and the directory synced (`atomic-write-file`), so a crash or a power loss leaves the old record or the new one,
//! never half of either. The temporary file (`.pairings.json.<random>`) is made in the folder the record is in, so the
//! folder is all the backup rules need to name. The crate renames before it syncs the directory, so an error can come
//! after the new record is already in place: the result of a write is therefore what a read would return (`reported`),
//! not the crate's error.
//!
//! The record holds the access token to the person's computer, so it stays out of the OS backups (decided by the operator,
//! 2026-11-01): after a restore the person pairs again, as with the provider keys. That is why it has a folder: the
//! folder is what is kept out, with whatever is in it, the temporary file of a write that a kill interrupted included. On
//! iOS every write marks the folder `NSURLIsExcludedFromBackupKey` (`platform::backup`) before it puts anything in it. A
//! mark that fails is a failed write (`data.pairings-write`) that has written nothing, so the page tries again, which
//! marks it again. On Android the rules in the Android library of `mobile/plugin` keep the folder out.
//!
//! Nothing here defends against another process of the same user, as the desktop's held directory does: on a phone the
//! file is in the app's own sandbox (the data directory only this app can open), and no other app can reach it.
use crate::platform::{app_files, backup, private_dir};
use atomic_write_file::AtomicWriteFile;
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Manager, Runtime};

/// The most a record may hold, read or written (the desktop's bound).
const LIMIT: u64 = 64 * 1024;

/// The folder of the record, in the app's own files: the record is the only thing in it, and the folder is what stays out
/// of the device backup. The Android backup rules (`mobile/plugin/android`) name it too.
const DIRECTORY: &str = "pairings";

/// The record's file name, in its folder.
const FILE: &str = "pairings.json";

type Answer = Result<Value, Value>;

fn refused(key: &str) -> Value {
    json!({ "key": key })
}

/// Whether `value` has the shape of pairings. The same check as the desktop's (`checked_pairings` in
/// `desktop/src/data/pairings.rs`): the room page's contract is one, on either host. The desktop's file is also built
/// without Tauri by `test-support`, which cannot reach a shared copy in `app-tauri` (it would link a webview), so the two
/// stay copies of each other until the shape has a home both can reach.
fn checked_pairings(value: &Value) -> bool {
    let Some(rows) = value.get("pairings").and_then(Value::as_array) else {
        return false;
    };
    let pending = value.get("pendingRevokes").and_then(Value::as_array);
    if rows.len() > 32
        || pending.is_some_and(|rows| rows.len() > 32)
        || !serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() as u64 <= LIMIT)
    {
        return false;
    }
    let valid = |row: &Value| {
        row.get("local").is_none()
            && ["fp", "public_key", "device_id", "token"]
                .iter()
                .all(|field| {
                    row.get(*field)
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.is_empty() && s.len() <= 4096)
                })
            && row
                .get("urls")
                .and_then(Value::as_array)
                .is_some_and(|urls| {
                    urls.len() <= 16
                        && urls
                            .iter()
                            .all(|url| url.as_str().is_some_and(|s| s.len() <= 2048))
                })
    };
    rows.iter().all(valid) && pending.is_none_or(|rows| rows.iter().all(valid))
}

/// The pairings file of the app.
pub struct PairingFile {
    directory: PathBuf,
    path: PathBuf,
}

impl PairingFile {
    /// The app's record, in `pairings/` in its own private files: Application Support's app folder on iOS, `filesDir` on
    /// Android (`platform::app_files`), from Tauri's path resolver. Nothing is created until the first write, which
    /// creates the folder, private to the app.
    pub fn of_app<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Self> {
        let mut files = app.path().app_data_dir()?;
        if let Some(own) = app_files::FILES {
            files.push(own);
        }
        Ok(Self::new(&files))
    }

    /// The record in the folder `pairings/` of `files`.
    fn new(files: &Path) -> Self {
        let directory = files.join(DIRECTORY);
        let path = directory.join(FILE);
        Self { directory, path }
    }

    /// The pairings as the page wrote them last, or the empty set when none were. A record that is too big, not JSON or
    /// not pairings is refused `data.pairings-invalid`, and is left as it is.
    pub fn read(&self) -> Answer {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(json!({ "pairings": [], "pendingRevokes": [] }))
            }
            Err(_) => return Err(refused("data.pairings-read")),
        };
        let mut bytes = Vec::new();
        // One byte past the bound tells a record that is too big from one that is exactly the bound.
        file.take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| refused("data.pairings-read"))?;
        if bytes.len() as u64 > LIMIT {
            return Err(refused("data.pairings-invalid"));
        }
        serde_json::from_slice::<Value>(&bytes)
            .ok()
            .filter(checked_pairings)
            .ok_or_else(|| refused("data.pairings-invalid"))
    }

    /// Keeps `value` after checking its shape and its size, in a folder kept out of the device backup; refuses what is
    /// not pairings (`data.pairings-invalid`), changing nothing, and what could not be kept (`data.pairings-write`), when
    /// a read still returns the old record.
    pub fn write(&self, value: &Value) -> Answer {
        self.write_excluding(value, backup::exclude_from_backup)
    }

    /// `write`, with the exclusion of the folder from the backup as a parameter: the tests watch it, as no host has a
    /// backup.
    fn write_excluding(
        &self,
        value: &Value,
        exclude: impl FnOnce(&Path) -> io::Result<()>,
    ) -> Answer {
        let bytes = serde_json::to_vec(value).map_err(|_| refused("data.pairings-invalid"))?;
        if !checked_pairings(value) || bytes.len() as u64 > LIMIT {
            return Err(refused("data.pairings-invalid"));
        }
        // The folder exists and is marked before the record, or the temporary file of its write, is put in it: no copy of
        // the token is ever outside the mark. The mark is on the folder, which the rename of a new file over the record
        // does not touch, and is set again on every write, so one that was missed (a kill between creating the folder and
        // marking it) or reset is set by the next.
        private_dir::create_private_dir(&self.directory)
            .and_then(|()| exclude(&self.directory))
            .map_err(|_| refused("data.pairings-write"))?;
        reported(write_atomically(&self.path, &bytes), &self.path, &bytes)
    }
}

/// Writes `bytes` to `path` all or nothing.
fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = AtomicWriteFile::open(path)?;
    file.write_all(bytes)?;
    file.commit()
}

/// What a write that ended in `result` did, as the page is told: the record on disk is `bytes` or it is not. `atomic-write-file`
/// renames the new record into place and then syncs the directory, so an error can leave the new record already visible;
/// that write is done, as far as a read can tell (whether the rename survives a power loss at that very moment is not known).
/// An error before the rename leaves the old record, which is `bytes` only if the page wrote the same record again.
fn reported(result: io::Result<()>, path: &Path, bytes: &[u8]) -> Answer {
    match result {
        Ok(()) => Ok(Value::Null),
        Err(_) if fs::read(path).is_ok_and(|on_disk| on_disk == bytes) => Ok(Value::Null),
        Err(_) => Err(refused("data.pairings-write")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn row(token: &str) -> Value {
        json!({ "fp": "fp", "public_key": "pk", "device_id": "id", "token": token, "urls": ["https://example.test"] })
    }

    /// The app's files, in `home`: the record is in `app/pairings/` there.
    fn file_in(home: &tempfile::TempDir) -> PairingFile {
        PairingFile::new(&home.path().join("app"))
    }

    /// The folder of the record in `file_in(home)`, and the record in it.
    fn folder_in(home: &tempfile::TempDir) -> PathBuf {
        home.path().join("app").join(DIRECTORY)
    }

    fn record_in(home: &tempfile::TempDir) -> PathBuf {
        folder_in(home).join(FILE)
    }

    /// The names in `folder`, sorted.
    fn names_in(folder: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_shape_is_the_desktops() {
        let mut local = row("t");
        local["local"] = json!(true);
        let mut empty = row("");
        empty["urls"] = json!(vec!["https://example.test"; 17]);
        assert!(checked_pairings(&json!({ "pairings": [] })));
        assert!(checked_pairings(
            &json!({ "pairings": [row("t")], "pendingRevokes": [row("u")], "future": true })
        ));
        for not in [
            json!({}),
            json!([]),
            json!({ "pairings": [{}] }),
            json!({ "pairings": [], "pendingRevokes": [{}] }),
            json!({ "pairings": [local] }),
            json!({ "pairings": [empty] }),
            json!({ "pairings": vec![row("t"); 33] }),
        ] {
            assert!(!checked_pairings(&not), "{not}");
        }
    }

    #[test]
    fn nothing_kept_is_the_empty_set_and_a_read_creates_nothing() {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        assert_eq!(
            store.read(),
            Ok(json!({ "pairings": [], "pendingRevokes": [] }))
        );
        assert!(!home.path().join("app").exists());
    }

    #[test]
    fn what_is_written_is_read_back_and_replaced_whole() {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        let first = json!({ "pairings": [row("a")], "pendingRevokes": [], "future": true });
        assert_eq!(store.write(&first), Ok(Value::Null));
        assert_eq!(store.read(), Ok(first));
        let next = json!({ "pairings": [] });
        assert_eq!(store.write(&next), Ok(Value::Null));
        assert_eq!(store.read(), Ok(next));
    }

    #[test]
    fn the_record_is_alone_in_a_private_folder_of_its_own() {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        store.write(&json!({ "pairings": [] })).unwrap();
        store.write(&json!({ "pairings": [row("t")] })).unwrap();
        // The record is the folder's only content, so the folder is all a backup rule has to name, and nothing but the
        // folder is put in the app's files.
        assert_eq!(names_in(&home.path().join("app")), [DIRECTORY]);
        assert_eq!(
            names_in(&folder_in(&home)),
            [FILE],
            "no temporary file is left behind"
        );
        assert!(private_dir::only_the_owner_can_open(&folder_in(&home)));
    }

    #[test]
    fn the_temporary_file_of_a_write_is_made_in_the_records_folder() {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        store.write(&json!({ "pairings": [] })).unwrap();
        // A write that a kill interrupts: the crate has made its temporary file next to the record and the process dies
        // before the rename, so nothing removes the file (`forget` skips the crate's cleanup, as a kill does).
        let mut interrupted = AtomicWriteFile::open(&store.path).unwrap();
        interrupted.write_all(b"the token").unwrap();
        std::mem::forget(interrupted);
        let left = names_in(&folder_in(&home));
        assert_eq!(left.len(), 2, "{left:?}");
        assert!(left.iter().any(|name| name == FILE), "{left:?}");
        assert!(
            left.iter().any(|name| name.starts_with(".pairings.json.")),
            "{left:?}"
        );
        // Outside the folder there is nothing for a backup to take.
        assert_eq!(names_in(&home.path().join("app")), [DIRECTORY]);
    }

    #[test]
    fn a_refused_write_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        let saved = json!({ "pairings": [row("kept")] });
        store.write(&saved).unwrap();
        let too_big = json!({ "pairings": [], "pad": "x".repeat(LIMIT as usize) });
        for invalid in [json!({}), json!({ "pairings": [{}] }), json!([]), too_big] {
            assert_eq!(store.write(&invalid), Err(refused("data.pairings-invalid")));
            assert_eq!(store.read(), Ok(saved.clone()));
        }
    }

    #[test]
    fn a_record_that_is_not_pairings_is_refused_and_left_as_it_is() {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        fs::create_dir_all(folder_in(&home)).unwrap();
        let path = record_in(&home);
        for bytes in [
            b"not json".to_vec(),
            b"{}".to_vec(),
            vec![b' '; LIMIT as usize + 1],
        ] {
            fs::write(&path, &bytes).unwrap();
            assert_eq!(store.read(), Err(refused("data.pairings-invalid")));
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn a_record_of_exactly_the_bound_is_kept() {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        let around = serde_json::to_vec(&json!({ "pairings": [], "pad": "" }))
            .unwrap()
            .len();
        let record = json!({ "pairings": [], "pad": "x".repeat(LIMIT as usize - around) });
        assert_eq!(serde_json::to_vec(&record).unwrap().len() as u64, LIMIT);
        assert_eq!(store.write(&record), Ok(Value::Null));
        assert_eq!(store.read(), Ok(record));
    }

    #[test]
    fn what_cannot_be_opened_or_kept_is_a_failure_not_an_invalid_record() {
        let home = tempfile::tempdir().unwrap();
        // The record's path is a directory: not a file to read, not one to replace.
        fs::create_dir_all(record_in(&home)).unwrap();
        let store = file_in(&home);
        assert_eq!(store.read(), Err(refused("data.pairings-read")));
        assert_eq!(
            store.write(&json!({ "pairings": [] })),
            Err(refused("data.pairings-write"))
        );
    }

    #[test]
    fn an_error_after_the_rename_is_reported_by_what_is_on_disk() {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        let old = json!({ "pairings": [row("old")] });
        let new = json!({ "pairings": [row("new")] });
        let bytes = serde_json::to_vec(&new).unwrap();
        let path = record_in(&home);
        let sync_failed = || Err(io::Error::other("the directory could not be synced"));
        store.write(&old).unwrap();
        assert_eq!(reported(Ok(()), &path, &bytes), Ok(Value::Null));
        // The new record is in place already: the error is the sync after the rename, and the write is done.
        fs::write(&path, &bytes).unwrap();
        assert_eq!(reported(sync_failed(), &path, &bytes), Ok(Value::Null));
        assert_eq!(store.read(), Ok(new));
        // The old record is still there: the write failed, and a read says so.
        store.write(&old).unwrap();
        assert_eq!(
            reported(sync_failed(), &path, &bytes),
            Err(refused("data.pairings-write"))
        );
        assert_eq!(store.read(), Ok(old));
        // Nothing is there at all.
        fs::remove_file(&path).unwrap();
        assert_eq!(
            reported(sync_failed(), &path, &bytes),
            Err(refused("data.pairings-write"))
        );
    }

    #[test]
    fn every_write_excludes_the_folder_before_anything_is_put_in_it() {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        let seen = RefCell::new(Vec::new());
        let watch = |at: &Path| -> io::Result<()> {
            seen.borrow_mut().push((at.to_owned(), names_in(at)));
            Ok(())
        };
        assert_eq!(
            store.write_excluding(&json!({ "pairings": [row("a")] }), watch),
            Ok(Value::Null)
        );
        assert_eq!(
            store.write_excluding(&json!({ "pairings": [] }), watch),
            Ok(Value::Null)
        );
        // Each time on the folder, which exists: empty for the first write, holding only the first record for the second.
        // The record, and the temporary file before it, are put in a folder that is marked already.
        let folder = folder_in(&home);
        assert_eq!(
            *seen.borrow(),
            [
                (folder.clone(), Vec::<String>::new()),
                (folder, vec![FILE.to_owned()])
            ]
        );
    }

    #[test]
    fn a_write_that_changes_nothing_excludes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        let never = |_: &Path| -> io::Result<()> { panic!("nothing was written") };
        assert_eq!(
            store.write_excluding(&json!({}), never),
            Err(refused("data.pairings-invalid"))
        );
        // A file where the folder should be: there is no folder to mark.
        fs::create_dir_all(home.path().join("app")).unwrap();
        fs::write(folder_in(&home), b"in the way").unwrap();
        assert_eq!(
            store.write_excluding(&json!({ "pairings": [] }), never),
            Err(refused("data.pairings-write"))
        );
    }

    #[test]
    fn an_exclusion_that_fails_fails_the_write_before_it_writes_and_the_next_write_marks_the_folder_again(
    ) {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        let old = json!({ "pairings": [row("old")] });
        let new = json!({ "pairings": [row("new")] });
        let failing = |_: &Path| -> io::Result<()> { Err(io::Error::other("no mark")) };
        // No record yet: the failed write left none, and no file in the unmarked folder.
        assert_eq!(
            store.write_excluding(&old, failing),
            Err(refused("data.pairings-write"))
        );
        assert_eq!(
            store.read(),
            Ok(json!({ "pairings": [], "pendingRevokes": [] }))
        );
        assert_eq!(names_in(&folder_in(&home)), Vec::<String>::new());
        let marked = Cell::new(0);
        let count = |_: &Path| -> io::Result<()> {
            marked.set(marked.get() + 1);
            Ok(())
        };
        assert_eq!(store.write_excluding(&old, count), Ok(Value::Null));
        assert_eq!(marked.get(), 1);
        // A record in place: a mark that fails changes nothing, and a read still returns the old record.
        assert_eq!(
            store.write_excluding(&new, failing),
            Err(refused("data.pairings-write"))
        );
        assert_eq!(store.read(), Ok(old));
        assert_eq!(store.write_excluding(&new, count), Ok(Value::Null));
        assert_eq!(marked.get(), 2);
        assert_eq!(store.read(), Ok(new));
    }

    #[test]
    fn a_file_where_a_directory_should_be_fails_the_write() {
        let home = tempfile::tempdir().unwrap();
        let record = json!({ "pairings": [] });
        // Where the app's files should be.
        fs::write(home.path().join("app"), b"in the way").unwrap();
        assert_eq!(
            file_in(&home).write(&record),
            Err(refused("data.pairings-write"))
        );
        // Where the record's folder should be.
        fs::remove_file(home.path().join("app")).unwrap();
        fs::create_dir(home.path().join("app")).unwrap();
        fs::write(folder_in(&home), b"in the way").unwrap();
        assert_eq!(
            file_in(&home).write(&record),
            Err(refused("data.pairings-write"))
        );
    }
}
