//! The setting and the installation id, in `crash-reports.json` in the app's own directory. A file that is missing is the
//! default (`auto`, no id yet): the app never asks, and never fails because of it. A file that is there but cannot be read
//! or understood is not "no choice": the person may have chosen `never`, so that run sends nothing (and says so in the log),
//! and the file is left as it is until the person chooses.
//!
//! The installation id is random (never derived from hardware or an account), pseudonymous, replaced after a year, and
//! removed when the person chooses `never`: nothing keeps identifying the installation once it stops reporting.

use super::Mode;
use crate::error::SharedError;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const FILE: &str = "crash-reports.json";
/// How long an installation id lasts.
pub const ID_LIFETIME_SECS: u64 = 365 * 24 * 60 * 60;
/// The most the file may hold when read.
const LIMIT: u64 = 4 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Install {
    id: String,
    /// Unix seconds.
    created: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    mode: Mode,
    #[serde(default)]
    install: Option<Install>,
}

pub struct Settings {
    path: PathBuf,
    stored: Stored,
}

/// 128 random bits as 32 hex digits, from the OS.
fn random_id() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| std::io::Error::other("no randomness"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn plain_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The bytes of a small file of the app's own (at most [`LIMIT`]): `Ok(None)` only when it does not exist.
pub(super) fn read_small(path: &Path) -> Result<Option<Vec<u8>>, std::io::ErrorKind> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        // Windows reports a path through a regular file as `NotFound` too, where Unix says `NotADirectory`: a parent
        // that is a regular file is never "no file yet".
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return if path.parent().is_some_and(|dir| dir.is_file()) { Err(error.kind()) } else { Ok(None) };
        }
        Err(error) => return Err(error.kind()),
    };
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(file, LIMIT + 1), &mut bytes).map_err(|error| error.kind())?;
    if bytes.len() as u64 > LIMIT {
        return Err(std::io::ErrorKind::InvalidData);
    }
    Ok(Some(bytes))
}

/// Atomically: a temporary file beside `path`, then a rename.
pub(super) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if path.as_os_str().is_empty() {
        return Err(std::io::Error::other("no place to keep the file"));
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let temporary = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)
}

/// The file: `Ok(None)` only when it does not exist.
fn read(path: &Path) -> Result<Option<Stored>, std::io::ErrorKind> {
    let Some(bytes) = read_small(path)? else { return Ok(None) };
    let mut stored = serde_json::from_slice::<Stored>(&bytes).map_err(|_| std::io::ErrorKind::InvalidData)?;
    if stored.install.as_ref().is_some_and(|install| !plain_id(&install.id)) {
        stored.install = None;
    }
    Ok(Some(stored))
}

impl Settings {
    /// What `dir` holds. No file is the default; a file that is there but unreadable, too big or not ours is `never` for
    /// this run (never written back: the person's next choice replaces it).
    pub fn load(dir: &Path) -> Settings {
        let path = dir.join(FILE);
        let stored = match read(&path) {
            Ok(Some(stored)) => stored,
            Ok(None) => Stored::default(),
            Err(kind) => {
                tracing::warn!(
                    error.type = SharedError::CrashSettingsUnreadable.code(),
                    error.kind = ?kind,
                    "the crash report setting could not be read; nothing is sent in this run"
                );
                Stored { mode: Mode::Never, install: None }
            }
        };
        Settings { path, stored }
    }

    /// No place to keep anything (the app's directory could not be had): `never` for this run, and nothing can be written.
    pub fn unavailable() -> Settings {
        Settings { path: PathBuf::new(), stored: Stored { mode: Mode::Never, install: None } }
    }

    pub fn mode(&self) -> Mode {
        self.stored.mode
    }

    /// Keeps `mode`. `never` also forgets the installation id. Nothing changes when the file cannot be written.
    pub fn set_mode(&mut self, mode: Mode) -> std::io::Result<()> {
        let mut next = self.stored.clone();
        next.mode = mode;
        if mode == Mode::Never {
            next.install = None;
        }
        self.write(&next)?;
        self.stored = next;
        Ok(())
    }

    /// The installation id as of `now` (Unix seconds): made on first use and again when a year old.
    pub fn install_id(&mut self, now: u64) -> std::io::Result<String> {
        if let Some(install) = &self.stored.install {
            if now.saturating_sub(install.created) < ID_LIFETIME_SECS
                && install.created <= now.saturating_add(ID_LIFETIME_SECS)
            {
                return Ok(install.id.clone());
            }
        }
        let mut next = self.stored.clone();
        let id = random_id()?;
        next.install = Some(Install { id: id.clone(), created: now });
        self.write(&next)?;
        self.stored = next;
        Ok(id)
    }

    fn write(&self, stored: &Stored) -> std::io::Result<()> {
        write_atomic(&self.path, &serde_json::to_vec(stored).map_err(std::io::Error::other)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_file_is_auto_and_a_file_that_cannot_be_understood_is_never_for_that_run() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Settings::load(dir.path()).mode(), Mode::Auto, "no file: the default");
        let file = dir.path().join(FILE);
        for broken in [b"not json".to_vec(), vec![b' '; 5000], br#"{"mode":"sometimes"}"#.to_vec()] {
            std::fs::write(&file, &broken).unwrap();
            assert_eq!(Settings::load(dir.path()).mode(), Mode::Never, "never silently re-enabled");
            assert_eq!(std::fs::read(&file).unwrap(), broken, "the file is left as it is");
        }
        // A directory where the file should be cannot be read either (not "not found").
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        assert_eq!(Settings::load(dir.path()).mode(), Mode::Never);
        // The person's next choice replaces whatever was there.
        std::fs::remove_dir(&file).unwrap();
        std::fs::write(&file, b"not json").unwrap();
        let mut settings = Settings::load(dir.path());
        settings.set_mode(Mode::Auto).unwrap();
        assert_eq!(Settings::load(dir.path()).mode(), Mode::Auto);
    }

    #[test]
    fn the_mode_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::load(dir.path());
        settings.set_mode(Mode::Never).unwrap();
        assert_eq!(Settings::load(dir.path()).mode(), Mode::Never);
        settings.set_mode(Mode::Auto).unwrap();
        assert_eq!(Settings::load(dir.path()).mode(), Mode::Auto);
    }

    #[test]
    fn the_install_id_is_random_stable_for_a_year_then_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::load(dir.path());
        let first = settings.install_id(1_000).unwrap();
        assert!(plain_id(&first));
        assert_eq!(settings.install_id(2_000).unwrap(), first);
        assert_eq!(Settings::load(dir.path()).install_id(3_000).unwrap(), first, "kept on disk");
        let second = settings.install_id(1_000 + ID_LIFETIME_SECS).unwrap();
        assert_ne!(second, first, "rotated after a year");
        assert_ne!(Settings::load(dir.path()).install_id(1_000 + ID_LIFETIME_SECS + 5).unwrap(), first);
    }

    #[test]
    fn never_forgets_the_install_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::load(dir.path());
        let first = settings.install_id(10).unwrap();
        settings.set_mode(Mode::Never).unwrap();
        let text = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert!(!text.contains(&first), "{text}");
        settings.set_mode(Mode::Auto).unwrap();
        assert_ne!(settings.install_id(11).unwrap(), first, "a new id after never");
    }

    #[test]
    fn a_failed_write_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where the temporary file goes: the write cannot happen.
        std::fs::create_dir(dir.path().join("crash-reports.json.tmp")).unwrap();
        let mut settings = Settings::load(dir.path());
        assert!(settings.set_mode(Mode::Never).is_err());
        assert_eq!(settings.mode(), Mode::Auto);
    }
}
