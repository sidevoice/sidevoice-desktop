//! Where the core and the connector listen (SEAMS §1, §2, §4): read from the records they publish, never worked out.
//! The core writes `endpoint` in `C/core.json`; the connector writes `{pid, endpoint}` in `D/connector.json` once it
//! listens and removes it when it exits cleanly. `endpoint` is the record `sidevoice-platform`'s `ipc` writes, parsed
//! back with its own type and handed on as it is.
//!
//! Both records are read as every file the app acts on is (`sidevoice_platform::fs::Directory`): through the directory's checked
//! descriptor, opened without following a link and without blocking, a regular file of this user's.
//!
//! A record that is not there, or a connector record whose `pid` is dead (it stays after a crash), is no host:
//! [`Check::Missing`]. One that is there but cannot be trusted or understood is a refusal.

use crate::checks::Check;
use crate::paths::DataDirs;
use crate::Refusal;
use serde_json::Value;
use sidevoice_platform::fs::{Access, Directory, HeldDirectory};
use sidevoice_platform::ipc::Endpoint;
use sidevoice_platform::process::{Process, Processes};
use std::io;
use std::path::Path;

const RECORD_LIMIT: u64 = 64 * 1024;

/// The core's endpoint, from `C/core.json`: `C` private to this user, the record private too (it holds the core's
/// token).
pub fn core(dirs: &DataDirs) -> Result<Endpoint, Check> {
    let dir = HeldDirectory::open(&dirs.core(), Access::Private)
        .map_err(|e| crate::checks::storage_error(&dirs.core(), "identity.unsafe-directory", e))?;
    let record = read(&dir, &dirs.core_record(), Access::Private, "identity.unsafe-directory")?;
    endpoint(&record, &dirs.core_record())
}

/// The connector's endpoint, from `D/connector.json`, when its `pid` is alive (a live process, not a zombie, as
/// `Processes::identity` observes it): `D` this user's and not writable by others, so is the record.
pub fn connector(dirs: &DataDirs) -> Result<Endpoint, Check> {
    let path = dirs.connector_record();
    let dir = HeldDirectory::open(&dirs.data, Access::NotWritableByOthers)
        .map_err(|e| crate::checks::storage_error(&dirs.data, "install.unsafe", e))?;
    let record = read(&dir, &path, Access::NotWritableByOthers, "install.unsafe")?;
    let pid = record.get("pid").and_then(Value::as_u64).and_then(|pid| u32::try_from(pid).ok()).filter(|pid| *pid > 0);
    match pid {
        Some(pid) if Processes::identity(pid).is_some() => endpoint(&record, &path),
        Some(_) => Err(Check::Missing),
        None => Err(invalid(&path, "no pid")),
    }
}

fn read(dir: &HeldDirectory, path: &Path, access: Access, key: &str) -> Result<Value, Check> {
    let bytes = dir
        .read(path.file_name().expect("record leaf"), access, RECORD_LIMIT)
        .map_err(|e| {
            if e.kind() == io::ErrorKind::InvalidData {
                invalid(path, "too large")
            } else {
                crate::checks::storage_error(path, key, e)
            }
        })?
        .ok_or(Check::Missing)?;
    serde_json::from_slice(&bytes).map_err(|e| invalid(path, &e.to_string()))
}

fn endpoint(record: &Value, path: &Path) -> Result<Endpoint, Check> {
    let text = record.get("endpoint").and_then(Value::as_str).ok_or_else(|| invalid(path, "no endpoint"))?;
    text.parse().map_err(|e: io::Error| invalid(path, &e.to_string()))
}

fn invalid(path: &Path, why: &str) -> Check {
    Check::Unsafe(Refusal::new(
        "endpoint.record-invalid",
        format!("{} does not say where it listens ({why}); the app does not connect to it.", path.display()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sidevoice_desktop_test_support::platform::storage::{StorageFixture, StorageFixtures};
    use sidevoice_platform::ipc::Address as _;

    fn dirs() -> (tempfile::TempDir, DataDirs) {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("d");
        std::fs::create_dir_all(data.join("core")).unwrap();
        StorageFixtures::private(&data);
        StorageFixtures::private(&data.join("core"));
        (tmp, DataDirs::new(data))
    }

    fn write(path: &Path, text: &str) {
        let _ = std::fs::remove_file(path);
        std::fs::write(path, text).unwrap();
        StorageFixtures::private(path);
    }

    #[test]
    fn absent_discovery_never_creates_and_unsafe_ancestry_is_not_absence() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = DataDirs::new(tmp.path().join("absent"));
        assert_eq!(core(&dirs), Err(Check::Missing));
        assert_eq!(connector(&dirs), Err(Check::Missing));
        assert!(!dirs.data.exists());
        StorageFixtures::writable(tmp.path());
        assert!(matches!(core(&dirs), Err(Check::Unsafe(_))));
        assert!(matches!(connector(&dirs), Err(Check::Unsafe(_))));
        assert!(!dirs.data.exists());
        StorageFixtures::private(tmp.path());
    }

    #[test]
    fn oversized_records_are_invalid_not_absent() {
        let (_tmp, dirs) = dirs();
        write(&dirs.core_record(), &" ".repeat(RECORD_LIMIT as usize + 1));
        assert!(matches!(core(&dirs), Err(Check::Unsafe(r)) if r.key == "endpoint.record-invalid"));
    }

    #[test]
    fn the_core_is_where_its_record_says_and_nowhere_when_it_has_none() {
        let (tmp, dirs) = dirs();
        assert_eq!(core(&dirs), Err(Check::Missing));
        let local = Endpoint::new(tmp.path(), "local");
        write(&dirs.core_record(), &json!({"pid": 1, "endpoint": local.to_string(), "token": "x"}).to_string());
        assert_eq!(core(&dirs), Ok(local));
        for bad in ["{}", r#"{"endpoint":"relative.sock"}"#, r#"{"endpoint":3}"#, "nope"] {
            write(&dirs.core_record(), bad);
            assert!(matches!(core(&dirs), Err(Check::Unsafe(r)) if r.key == "endpoint.record-invalid"), "{bad}");
        }
    }

    #[test]
    fn the_connector_is_where_its_record_says_while_its_pid_is_alive() {
        let (tmp, dirs) = dirs();
        assert_eq!(connector(&dirs), Err(Check::Missing));
        let me = std::process::id();
        let endpoint = Endpoint::new(tmp.path(), "connector");
        write(&dirs.connector_record(), &json!({"pid": me, "endpoint": endpoint.to_string()}).to_string());
        assert_eq!(connector(&dirs), Ok(endpoint));
        // A crashed connector leaves its record: a pid nobody has is stale. (This test binary, asked only to list its
        // tests.)
        let mut gone = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let dead = gone.id();
        gone.wait().unwrap();
        write(&dirs.connector_record(), &format!(r#"{{"pid":{dead},"endpoint":"/run/user/1/x/connector.sock"}}"#));
        assert_eq!(connector(&dirs), Err(Check::Missing));
        write(&dirs.connector_record(), r#"{"endpoint":"/run/user/1/x/connector.sock"}"#);
        assert!(matches!(connector(&dirs), Err(Check::Unsafe(_))));
    }

    #[test]
    fn a_record_that_is_not_a_plain_private_file_is_refused_without_blocking() {
        let (tmp, dirs) = dirs();
        let me = std::process::id();
        let body = format!(r#"{{"pid":{me},"endpoint":"/run/user/1/x/connector.sock"}}"#);
        // A named pipe in its place: opened without blocking and refused as not a regular file.
        StorageFixtures::special_record(&dirs.connector_record());
        assert!(matches!(connector(&dirs), Err(Check::Unsafe(_))));
        std::fs::remove_file(dirs.connector_record()).unwrap();
        // A link to a good file is not followed.
        let real = tmp.path().join("real.json");
        write(&real, &body);
        StorageFixtures::symlink(&real, dirs.connector_record());
        assert!(matches!(connector(&dirs), Err(Check::Unsafe(_))));
        // Writable by others: refused.
        std::fs::remove_file(dirs.connector_record()).unwrap();
        write(&dirs.connector_record(), &body);
        StorageFixtures::writable(&dirs.connector_record());
        assert!(matches!(connector(&dirs), Err(Check::Unsafe(_))));
        // The core's record: a link, or readable by others, is refused too.
        write(&real, r#"{"endpoint":"/run/user/1/x/local.sock"}"#);
        StorageFixtures::symlink(&real, dirs.core_record());
        assert!(matches!(core(&dirs), Err(Check::Unsafe(_))));
        std::fs::remove_file(dirs.core_record()).unwrap();
        write(&dirs.core_record(), r#"{"endpoint":"/run/user/1/x/local.sock"}"#);
        StorageFixtures::readable(&dirs.core_record());
        assert!(matches!(core(&dirs), Err(Check::Unsafe(_))));
    }
}
