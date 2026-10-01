//! `local-host.json` in the app's config directory (SEAMS §5): the app's pairing with this computer's core, the only
//! copy of its device token. Written 0600, atomically; one writer, the app's native side. A file that is missing or
//! unreadable is no pairing: the app pairs again (which revokes the previous local device in the core).

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const FILE_NAME: &str = "local-host.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pairing {
    /// The core's fingerprint when the app paired.
    pub fp: String,
    /// The key the core proves itself with (SPKI DER, base64), pinned.
    pub public_key: String,
    pub device_id: String,
    /// The device token: sent only over the core's socket, never to the page.
    pub token: String,
    /// The machine's name, as the core calls itself.
    pub host: String,
    /// ISO-8601, UTC.
    pub paired_at: String,
}

impl Pairing {
    /// The token, kept out of `{:?}`.
    pub fn redacted(&self) -> String {
        format!("{{fp: {}, device_id: {}, host: {}, paired_at: {}}}", self.fp, self.device_id, self.host, self.paired_at)
    }
}

pub fn load(dir: &Path) -> Option<Pairing> {
    serde_json::from_slice(&fs::read(dir.join(FILE_NAME)).ok()?).ok()
}

/// Writes the pairing next to the file, 0600, synced, then renames it over: a crash leaves the old file or the new.
pub fn save(dir: &Path, pairing: &Pairing) -> io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let staged = dir.join(format!(".{FILE_NAME}.{}", std::process::id()));
    let written = (|| {
        let _ = fs::remove_file(&staged);
        let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&staged)?;
        file.write_all(&serde_json::to_vec_pretty(pairing).map_err(io::Error::other)?)?;
        file.sync_all()?;
        fs::rename(&staged, dir.join(FILE_NAME))
    })();
    if written.is_err() {
        let _ = fs::remove_file(&staged);
    }
    written
}

pub fn remove(dir: &Path) -> io::Result<()> {
    match fs::remove_file(dir.join(FILE_NAME)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Now, as ISO-8601 UTC to the second (`2026-10-01T12:00:00Z`).
pub fn now_iso() -> String {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    iso(seconds)
}

fn iso(seconds: u64) -> String {
    let (days, rest) = ((seconds / 86_400) as i64, seconds % 86_400);
    // Days since the epoch to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn pairing() -> Pairing {
        Pairing {
            fp: "fp".into(),
            public_key: "key".into(),
            device_id: "dev".into(),
            token: "secret-token".into(),
            host: "MacBook".into(),
            paired_at: now_iso(),
        }
    }

    #[test]
    fn saved_0600_and_read_back() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("config");
        assert_eq!(load(&dir), None);
        save(&dir, &pairing()).unwrap();
        assert_eq!(load(&dir), Some(pairing()).map(|p| Pairing { paired_at: load(&dir).unwrap().paired_at, ..p }));
        let mode = fs::metadata(dir.join(FILE_NAME)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "nothing staged is left");
        assert!(!pairing().redacted().contains("secret-token"));
        remove(&dir).unwrap();
        remove(&dir).unwrap();
        assert_eq!(load(&dir), None);
    }

    #[test]
    fn a_corrupt_file_is_no_pairing() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(FILE_NAME), b"{not json").unwrap();
        assert_eq!(load(tmp.path()), None);
    }

    #[test]
    fn a_failed_write_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let blocked = tmp.path().join("config");
        fs::write(&blocked, b"a file where the directory should be").unwrap();
        assert!(save(&blocked, &pairing()).is_err());
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_759_320_000), "2025-10-01T12:00:00Z");
        assert_eq!(iso(951_782_400), "2000-02-29T00:00:00Z");
    }
}
