// Existing OS debt is capped by platform-debt.txt.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

//! Where the core and the connector listen (SEAMS §1, §2, §4): read from the records they publish, never worked out.
//! The core writes `endpoint` in `C/core.json`; the connector writes `{pid, endpoint}` in `D/connector.json` once it
//! listens and removes it when it exits cleanly. `endpoint` is the record `sidevoice-platform`'s `ipc` writes (an
//! absolute socket path on Unix, in the user's runtime directory), parsed back with its own type.
//!
//! Both records are read as every file the app acts on is (`trusted::Dir`): through the directory's checked
//! descriptor, opened without following a link and without blocking, a regular file of this user's.
//!
//! A record that is not there, or a connector record whose `pid` is dead (it stays after a crash), is no host:
//! [`Check::Missing`]. One that is there but cannot be trusted or understood is a refusal.

use crate::checks::Check;
use crate::paths::DataDirs;
use crate::trusted::Dir;
use crate::Refusal;
use serde_json::Value;
use sidevoice_platform::ipc::Endpoint;
use std::io;
use std::path::{Path, PathBuf};

const RECORD_LIMIT: u64 = 64 * 1024;

/// The core's socket, from `C/core.json`: `C` private to this user, the record private too (it holds the core's token).
pub fn core(dirs: &DataDirs) -> Result<PathBuf, Check> {
    let dir = Dir::open(&dirs.core(), 0o077, "identity.unsafe-directory")?;
    let record = read(&dir, "core.json", 0o077)?;
    socket(&record, &dirs.core_record())
}

/// The connector's socket, from `D/connector.json`, when its `pid` is alive: `D` this user's and not writable by
/// others, so is the record.
pub fn connector(dirs: &DataDirs) -> Result<PathBuf, Check> {
    let path = dirs.connector_record();
    let dir = Dir::open(&dirs.data, 0o022, "install.unsafe")?;
    let record = read(&dir, "connector.json", 0o022)?;
    let pid = record.get("pid").and_then(Value::as_u64).and_then(|pid| i32::try_from(pid).ok()).filter(|pid| *pid > 0);
    match pid {
        Some(pid) if alive(pid) => socket(&record, &path),
        Some(_) => Err(Check::Missing),
        None => Err(invalid(&path, "no pid")),
    }
}

fn read(dir: &Dir, name: &str, mask: u32) -> Result<Value, Check> {
    let path = dir.path().join(name);
    let bytes = dir.read(name, mask, RECORD_LIMIT + 1)?;
    if bytes.len() as u64 > RECORD_LIMIT {
        return Err(invalid(&path, "too large"));
    }
    serde_json::from_slice(&bytes).map_err(|e| invalid(&path, &e.to_string()))
}

fn socket(record: &Value, path: &Path) -> Result<PathBuf, Check> {
    let text = record.get("endpoint").and_then(Value::as_str).ok_or_else(|| invalid(path, "no endpoint"))?;
    let endpoint: Endpoint = text.parse().map_err(|e: io::Error| invalid(path, &e.to_string()))?;
    Ok(PathBuf::from(endpoint.to_string()))
}

fn invalid(path: &Path, why: &str) -> Check {
    Check::Unsafe(Refusal::new(
        "endpoint.record-invalid",
        format!("{} does not say where it listens ({why}); the app does not connect to it.", path.display()),
    ))
}

/// Whether process `pid` exists (signal 0: nothing is sent). Not permitted still means it is there.
fn alive(pid: i32) -> bool {
    // SAFETY: kill with signal 0 only checks; it has no other effect.
    unsafe { libc::kill(pid, 0) == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    fn dirs() -> (tempfile::TempDir, DataDirs) {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("d");
        std::fs::create_dir_all(data.join("core")).unwrap();
        chmod(&data, 0o700);
        chmod(&data.join("core"), 0o700);
        (tmp, DataDirs::new(data))
    }

    fn write(path: &Path, text: &str) {
        let _ = std::fs::remove_file(path);
        std::fs::write(path, text).unwrap();
        chmod(path, 0o600);
    }

    #[test]
    fn the_core_is_where_its_record_says_and_nowhere_when_it_has_none() {
        let (_tmp, dirs) = dirs();
        assert_eq!(core(&dirs), Err(Check::Missing));
        write(&dirs.core_record(), r#"{"pid":1,"endpoint":"/run/user/1/sidevoice-ab/local.sock","token":"x"}"#);
        assert_eq!(core(&dirs), Ok(PathBuf::from("/run/user/1/sidevoice-ab/local.sock")));
        for bad in ["{}", r#"{"endpoint":"relative.sock"}"#, r#"{"endpoint":3}"#, "nope"] {
            write(&dirs.core_record(), bad);
            assert!(matches!(core(&dirs), Err(Check::Unsafe(r)) if r.key == "endpoint.record-invalid"), "{bad}");
        }
    }

    #[test]
    fn the_connector_is_where_its_record_says_while_its_pid_is_alive() {
        let (_tmp, dirs) = dirs();
        assert_eq!(connector(&dirs), Err(Check::Missing));
        let me = std::process::id();
        write(
            &dirs.connector_record(),
            &format!(r#"{{"pid":{me},"endpoint":"/run/user/1/sidevoice-ab/connector.sock"}}"#),
        );
        assert_eq!(connector(&dirs), Ok(PathBuf::from("/run/user/1/sidevoice-ab/connector.sock")));
        // A crashed connector leaves its record: a pid nobody has is stale.
        let mut gone = std::process::Command::new("true").spawn().unwrap();
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
        let fifo = std::ffi::CString::new(dirs.connector_record().to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(matches!(connector(&dirs), Err(Check::Unsafe(_))));
        std::fs::remove_file(dirs.connector_record()).unwrap();
        // A link to a good file is not followed.
        let real = tmp.path().join("real.json");
        write(&real, &body);
        std::os::unix::fs::symlink(&real, dirs.connector_record()).unwrap();
        assert!(matches!(connector(&dirs), Err(Check::Unsafe(_))));
        // Writable by others: refused.
        std::fs::remove_file(dirs.connector_record()).unwrap();
        write(&dirs.connector_record(), &body);
        chmod(&dirs.connector_record(), 0o666);
        assert!(matches!(connector(&dirs), Err(Check::Unsafe(_))));
        // The core's record: a link, or readable by others, is refused too.
        write(&real, r#"{"endpoint":"/run/user/1/x/local.sock"}"#);
        std::os::unix::fs::symlink(&real, dirs.core_record()).unwrap();
        assert!(matches!(core(&dirs), Err(Check::Unsafe(_))));
        std::fs::remove_file(dirs.core_record()).unwrap();
        write(&dirs.core_record(), r#"{"endpoint":"/run/user/1/x/local.sock"}"#);
        chmod(&dirs.core_record(), 0o644);
        assert!(matches!(core(&dirs), Err(Check::Unsafe(_))));
    }
}
