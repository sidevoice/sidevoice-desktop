//! Preparing this computer from the app, without Node (docs/LOCAL_HOST.md → Install): what `npx sidevoice@latest
//! install` does, with the npm package fetched directly.
//!
//! 1. `download`: the latest release and this computer's platform package, from the registry ([`npm`]), and its
//!    tarball into a fresh private directory ([`Scratch`], made where [`staging_root`] says: the system's temporary
//!    directory, or on Windows a folder beside `D`), its bytes counted as they come. Cancellable.
//! 2. `verification`: the tarball's SHA-512 against the registry's integrity; a mismatch is refused
//!    (`install.checksum`) and nothing of it is unpacked. Cancellable.
//! 3. `staging`: the tarball unpacked ([`tarball`]), then the package's own installer run from it,
//!    `package/bin/sidevoice-connector install --json` with `SIDEVOICE_DATA_DIR` set to `D`: it stages the release under
//!    its own root with its core, writes `install.json`, defines and starts the service jobs, waits for them (60 s;
//!    an update that does not start is rolled back) and registers the agents. Cancellable until the installer starts,
//!    never after; it has [`Options::installer_time`], and is killed past it.
//! 4. `service-start`: from when the installer has written `install.json` (its staging done) until it answers.
//! 5. `pairing`: the host's ([`crate::host::LocalHost`]): the app pairs with the new core as on any core launch.
//!
//! The private directory is removed whatever happens. A failure is `{key, step?, params?, log_tail?, message}`
//! ([`Failure`]): the keys sidevoice-web knows where one fits (`local-host-install.ts`), the installer's own keys
//! through ([`installer_failure`]). A local file operation that fails also names, in `params`, what it was at
//! ([`Where`]), why ([`Reason`]) and the OS's own code (`os_error`), so the app log says what failed without a path.

use crate::cli::Cli;
use crate::npm::{self, Fetch, Minor};
use crate::platform::target::current_package;
use crate::tarball;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha512};
use sidevoice_platform::fs::{Access, Create, Directory, FileId, Files, Fs, HeldDirectory};
use sidevoice_platform::removal::Removal;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

/// What a second install is refused with while one runs (and what the installer's own lock, `service.busy`, becomes).
pub const BUSY: &str = "install.busy";
/// What an install the person cancelled rejects with.
pub const CANCELLED: &str = "install.cancelled";
/// What an update is stopped with, before its installer runs, when [`Options::gate`] says a call is on (or that the
/// count of calls is not known): nothing was changed, and it is not a failure (the update runs again once the call ends).
pub const WAITING_CALL: &str = "update.waiting-call";
/// What an update is stopped with, likewise, when the person has stopped the service meanwhile (the installer would
/// start it).
pub const STOPPED: &str = "update.stopped-by-person";
/// What an update is stopped with, likewise, when the service was uninstalled meanwhile (nothing is left to update).
pub const GONE: &str = "update.not-installed";

/// Whether `key` is one of those an update is stopped with before its installer, by [`Options::gate`]: nothing changed,
/// not a failure.
pub fn is_held(key: &str) -> bool {
    matches!(key, WAITING_CALL | STOPPED | GONE)
}
/// The installer inside the package, where the installer itself expects to be (`<package>/bin/`), named as this OS
/// names a program (`bin/sidevoice-connector.exe` on Windows).
pub fn bundled_installer() -> String {
    sidevoice_platform::target::exe_name("bin/sidevoice-connector")
}

/// The installer's path in the package's archive.
pub(crate) fn installer_entry() -> String {
    format!("{}/{}", tarball::ROOT, bundled_installer())
}

/// Where an install is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Step {
    Download,
    Verification,
    Staging,
    ServiceStart,
    Pairing,
}

/// A failed install as the page gets it: a stable `key`, the step it failed at, what the page may say with it
/// (`params`: `check` for `install.authenticity`; `where`, `reason`, `os_error` and `root` for the app's own local file
/// failures, [`Failure::at`]; the installer's own), the installer's last lines of error output,
/// and an English sentence for a client that does not know the key.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Failure {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<Step>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_tail: Option<Vec<String>>,
    pub message: String,
}

impl Failure {
    pub fn new(key: &str, step: impl Into<Option<Step>>, message: impl Into<String>) -> Failure {
        Failure { key: key.to_string(), step: step.into(), params: None, log_tail: None, message: message.into() }
    }

    /// The check that failed, for `install.authenticity` (sidevoice-web's `INSTALL_CHECKS`).
    pub fn check(self, check: &str) -> Failure {
        self.param("check", json!(check))
    }

    /// `params[name]` = `value`, beside the params already there.
    fn param(mut self, name: &str, value: Value) -> Failure {
        let mut params = self.params.take().filter(Value::is_object).unwrap_or_else(|| json!({}));
        params[name] = value;
        self.params = Some(params);
        self
    }

    /// What failed, for support: `params.where` ([`Where`]), `params.reason` ([`Reason`]) and, when the OS gave one,
    /// `params.os_error`, its raw code. Closed sets and a number, never a path, so the app log keeps them
    /// ([`Failure::log_report`]); the path stays in `message`.
    pub fn at(self, place: Where, reason: Reason, os_error: Option<i32>) -> Failure {
        let failure = self.param("where", json!(place)).param("reason", json!(reason));
        match os_error {
            Some(code) => failure.param("os_error", json!(code)),
            None => failure,
        }
    }

    pub fn cancelled(step: Step) -> Failure {
        Failure::new(CANCELLED, step, "The install was cancelled.")
    }

    /// A local file operation at `place` failed: a full disk is `install.disk`, anything else `install.failed`; with
    /// [`Failure::at`] from the error.
    pub fn io(e: &io::Error, step: Step, place: Where, what: &str) -> Failure {
        let full = matches!(e.kind(), io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded);
        Failure::new(if full { "install.disk" } else { "install.failed" }, step, format!("{what}: {e}")).at(
            place,
            Reason::of(e),
            os_code(e),
        )
    }

    /// A directory at `place` that the shared private-directory policy refused, or that could not be checked:
    /// `install.unsafe`, with [`Failure::at`] from the error.
    pub fn unsafe_dir(e: &io::Error, step: Step, place: Where, what: &str) -> Failure {
        Failure::new("install.unsafe", step, format!("{what}: {e}")).at(place, Reason::of(e), os_code(e))
    }
}

/// What a local file operation of the install was at when it failed (`params.where`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Where {
    /// The directory the private directory is made in ([`StagingRoot`]).
    StagingRoot,
    /// The private directory itself, as made and checked.
    Scratch,
    /// The downloaded tarball in it.
    Download,
    /// What the tarball unpacks to.
    Package,
    /// The package's installer, checked before it runs.
    Installer,
}

impl Where {
    const ALL: [Where; 5] = [Where::StagingRoot, Where::Scratch, Where::Download, Where::Package, Where::Installer];
}

/// Why a local file operation failed (`params.reason`). The shared policy's refusals (`sidevoice_platform::fs`,
/// `PermissionDenied`) say why only in their English text, so until it gives a typed reason they are read from the
/// phrases of the pinned release ([`Reason::of`]); a refusal it words otherwise is `refused`, never guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    /// The path is not absolute.
    NotAbsolute,
    /// The path is not UTF-8, so it cannot be run.
    NotUtf8,
    /// A directory on the way, or the directory itself, belongs to another user.
    OwnedByOther,
    /// Somebody else may delete or rename what is in a directory on the way (Windows: an inherited
    /// "Authenticated Users: Modify", as below a drive root).
    OthersCanReplace,
    /// The directory itself is open to others.
    OpenToOthers,
    /// A directory on the way has an owner or permissions the policy refuses (Unix, which does not say which).
    UnsafeAncestor,
    /// A link, junction, mount point or other reparse point the policy does not follow, or not an ordinary directory.
    Link,
    /// On a network or shared drive.
    Network,
    /// Refused by the policy for a reason worded otherwise.
    Refused,
    /// Not there.
    NotFound,
    /// The OS refused access (`os_error` says how).
    AccessDenied,
    /// Already there.
    AlreadyExists,
    /// The disk or the quota is full.
    DiskFull,
    /// A read-only file system.
    ReadOnly,
    /// Any other OS error (`os_error` says which).
    Os,
}

impl Reason {
    const ALL: [Reason; 15] = [
        Reason::NotAbsolute,
        Reason::NotUtf8,
        Reason::OwnedByOther,
        Reason::OthersCanReplace,
        Reason::OpenToOthers,
        Reason::UnsafeAncestor,
        Reason::Link,
        Reason::Network,
        Reason::Refused,
        Reason::NotFound,
        Reason::AccessDenied,
        Reason::AlreadyExists,
        Reason::DiskFull,
        Reason::ReadOnly,
        Reason::Os,
    ];

    /// Why `e` happened: by its kind, and for a refusal (`PermissionDenied`) by the policy's wording, checked from the
    /// most specific; a `PermissionDenied` the policy did not word but the OS gave a code for is `access-denied`.
    pub fn of(e: &io::Error) -> Reason {
        let text = e.to_string();
        let says = |phrases: &[&str]| phrases.iter().any(|phrase| text.contains(phrase));
        match e.kind() {
            io::ErrorKind::NotFound => Reason::NotFound,
            io::ErrorKind::AlreadyExists => Reason::AlreadyExists,
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => Reason::DiskFull,
            io::ErrorKind::ReadOnlyFilesystem => Reason::ReadOnly,
            io::ErrorKind::InvalidInput if says(&["is not an absolute path"]) => Reason::NotAbsolute,
            io::ErrorKind::PermissionDenied => {
                if says(&["unsafe link", "a link of another user's", "not an ordinary directory", ": a link"]) {
                    Reason::Link
                } else if says(&["a network or shared drive"]) {
                    Reason::Network
                } else if says(&["owned by another user", "not this user's"]) {
                    Reason::OwnedByOther
                } else if says(&["others can replace what is in it"]) {
                    Reason::OthersCanReplace
                } else if says(&["open to others"]) {
                    Reason::OpenToOthers
                } else if says(&["unsafe ancestor owner or permissions"]) {
                    Reason::UnsafeAncestor
                } else if os_code(e).is_some() {
                    Reason::AccessDenied
                } else {
                    Reason::Refused
                }
            }
            _ => Reason::Os,
        }
    }

    /// Whether this is the policy refusing a directory (`install.unsafe`), not the OS failing an operation.
    fn is_refusal(self) -> bool {
        matches!(
            self,
            Reason::NotAbsolute
                | Reason::NotUtf8
                | Reason::OwnedByOther
                | Reason::OthersCanReplace
                | Reason::OpenToOthers
                | Reason::UnsafeAncestor
                | Reason::Link
                | Reason::Network
                | Reason::Refused
        )
    }
}

/// The OS's own code for `e`: its raw code, else the last `(os error N)` in its text (the shared policy rewraps OS
/// errors with the path, keeping their kind and text but not their code).
fn os_code(e: &io::Error) -> Option<i32> {
    e.raw_os_error().or_else(|| {
        let text = e.to_string();
        let from = text.rfind("(os error ")? + "(os error ".len();
        text[from..].split_once(')')?.0.parse().ok()
    })
}

impl Failure {
    /// What the app log records of a failed install: the key, the step and the known-safe params, structured fields
    /// only. The installer's message, its other params and its error output are free text from outside: they are not
    /// logged (the key says what failed); the page gets the failure whole, not through here. The key is logged only
    /// if it has the shape of one, and the params only by an allowlist of names whose values match their grammar ([`param_value_ok`]).
    pub fn log_report(&self) -> String {
        let key_ok = !self.key.is_empty()
            && self.key.len() <= 64
            && self.key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'));
        let mut report = if key_ok { self.key.clone() } else { "[invalid key]".to_string() };
        if let Some(step) = self.step {
            report.push_str(&format!(" at step {}", json!(step).as_str().unwrap_or("?")));
        }
        if let Some(params) = self.params.as_ref().and_then(Value::as_object) {
            let mut logged = Vec::new();
            let mut withheld = 0;
            for (name, value) in params {
                match value {
                    Value::String(text) if param_value_ok(name, text) => logged.push(format!("{name}={text}")),
                    Value::Number(n) if (name == "status" && n.is_u64()) || (name == "os_error" && n.is_i64()) => {
                        logged.push(format!("{name}={n}"))
                    }
                    _ => withheld += 1,
                }
            }
            if !logged.is_empty() {
                report.push_str(&format!(" ({})", logged.join(", ")));
            }
            if withheld > 0 {
                report.push_str(&format!(" [{withheld} param(s) not logged]"));
            }
        }
        report
    }
}

/// The `check` values a failure carries: sidevoice-web's `INSTALL_CHECKS`, the closed set the installer emits.
const CHECKS: &[&str] = &[
    "sha256",
    "sigstore",
    "sigstore-bundle",
    "issuer",
    "workflow",
    "source",
    "repository-id",
    "runner",
    "build-config",
    "predicate",
    "subject",
    "manifest",
    "developer-override",
    "platform",
    "download",
    "download-size",
    "archive-path",
    "archive-link",
    "archive-size",
    "archive-type",
];

/// A release version as the connector lets one be (`valid_version`: `[A-Za-z0-9.+_-]`, at most 64 characters, plus
/// `+` and 12 hex digits on a collision): a number first (`1`, `1.2`, `1.2.3`), the rest from that alphabet, at most 96.
fn is_version(text: &str) -> bool {
    text.len() <= 96
        && text.starts_with(|c: char| c.is_ascii_digit())
        && text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '_' | '-'))
}

/// A digest: lowercase hex of a SHA-1 (npm `shasum`), SHA-256 or SHA-512 length.
fn is_digest(text: &str) -> bool {
    matches!(text.len(), 40 | 64 | 128) && text.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// Whether `text` is how one of `values` serialises.
fn one_of<T: Serialize>(values: &[T], text: &str) -> bool {
    values.iter().any(|value| json!(value) == text)
}

/// A value is logged only if it has its param's grammar (never sanitized): `check` one of [`CHECKS`]; `version` a
/// release version; `expected` and `found` a version or a digest; `where`, `reason` and `root` one of [`Where`],
/// [`Reason`] and [`Root`]; `status` and `os_error` are logged only as numbers (the connector sends no word for
/// `status`).
fn param_value_ok(name: &str, text: &str) -> bool {
    match name {
        "check" => CHECKS.contains(&text),
        "version" => is_version(text),
        "expected" | "found" => is_version(text) || is_digest(text),
        "where" => one_of(&Where::ALL, text),
        "reason" => one_of(&Reason::ALL, text),
        "root" => one_of(&Root::ALL, text),
        _ => false,
    }
}

/// The install id as logged: the page's id is caller-supplied, so it is logged only if it has the shape of an id
/// (`[A-Za-z0-9_-]{1,64}`), else `[invalid job]`.
pub fn loggable_job(id: &str) -> &str {
    let ok = (1..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
    if ok {
        id
    } else {
        "[invalid job]"
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.key, self.message)
    }
}

/// `{step, done, total, cancellable}`: the bytes so far and of all (`null` when unknown, and outside `download`), and
/// whether a cancel would still be honoured.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Progress {
    pub step: Step,
    pub done: Option<u64>,
    pub total: Option<u64>,
    pub cancellable: bool,
}

/// One install, under the page's id for it: where it is, and whether it was asked to stop.
#[derive(Debug)]
pub struct Job {
    id: String,
    /// The app's own update of the service, not an install the person asked for: the service keeps its state meanwhile
    /// (`installing` is for the person's), and nothing is paired again that the person has not asked for.
    background: bool,
    state: Mutex<JobState>,
}

#[derive(Debug)]
struct JobState {
    progress: Progress,
    cancelled: bool,
}

impl Job {
    pub fn new(id: &str) -> Job {
        Job::made(id, false)
    }

    /// The app's own update of the service (see the field).
    pub fn background(id: &str) -> Job {
        Job::made(id, true)
    }

    fn made(id: &str, background: bool) -> Job {
        let progress = Progress { step: Step::Download, done: None, total: None, cancellable: true };
        Job { id: id.to_string(), background, state: Mutex::new(JobState { progress, cancelled: false }) }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn is_background(&self) -> bool {
        self.background
    }

    fn lock(&self) -> MutexGuard<'_, JobState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn progress(&self) -> Progress {
        self.lock().progress.clone()
    }

    pub fn step(&self) -> Step {
        self.lock().progress.step
    }

    /// Asks it to stop: true while that is honoured (and again once asked), false once the installer runs.
    pub fn cancel(&self) -> bool {
        let mut state = self.lock();
        if state.progress.cancellable || state.cancelled {
            state.cancelled = true;
        }
        state.cancelled
    }

    pub fn cancelled(&self) -> bool {
        self.lock().cancelled
    }

    /// On to `step` (no bytes yet), unless a cancel came first.
    pub fn enter(&self, step: Step) -> Result<(), Failure> {
        let mut state = self.lock();
        if state.cancelled {
            return Err(Failure::cancelled(state.progress.step));
        }
        state.progress = Progress { step, done: None, total: None, cancellable: state.progress.cancellable };
        Ok(())
    }

    fn bytes(&self, done: u64, total: Option<u64>) -> Result<(), Failure> {
        let mut state = self.lock();
        if state.cancelled {
            return Err(Failure::cancelled(state.progress.step));
        }
        state.progress.done = Some(done);
        state.progress.total = total;
        Ok(())
    }

    /// From here on nothing is cancelled: a cancel asked before stops it here instead, atomically with the switch.
    fn commit(&self) -> Result<(), Failure> {
        let mut state = self.lock();
        if state.cancelled {
            return Err(Failure::cancelled(state.progress.step));
        }
        state.progress.cancellable = false;
        Ok(())
    }
}

/// How an install goes, and its bounds.
#[derive(Debug, Clone)]
pub struct Options {
    /// This computer's platform package; none has no release (`install.no-bundle`).
    pub package: Option<String>,
    /// The system's temporary directory: where the private directory is made unless `temp_often_redirected`
    /// ([`staging_root`]).
    pub temp: PathBuf,
    /// Whether this OS's temporary directory is often redirected to a place other users can change
    /// ([`crate::platform::staging`]): then the private directory is made beside `D` instead ([`staging_root`]).
    pub temp_often_redirected: bool,
    /// How long ago a private directory left in the staging folder beside `D` must have last changed to be removed
    /// before an install ([`Scratch::create`]).
    pub stale_after: Duration,
    /// The tarball's largest size, and how long its download may take in all.
    pub download_limit: u64,
    pub download_time: Duration,
    pub unpack: tarball::Limits,
    /// How long the installer may run before it is killed. Its own bounds: 30 s for the install lock, 30 s for the
    /// staged binary's self-check, 60 s to verify the service, 60 s more to verify a rollback.
    pub installer_time: Duration,
    /// How long the host waits for the new core to be reachable (paired) before it answers the state anyway.
    pub settle_time: Duration,
    /// The minor the app works with: the release is the newest patch of it, not the latest `sidevoice` (which may be a
    /// newer minor, one the app does not speak). `None`: the latest.
    pub minor: Option<Minor>,
    /// Asked once the release is verified and unpacked, right before its installer would restart the service: an
    /// update that is told why not ([`WAITING_CALL`], [`STOPPED`], [`GONE`]) stops there with that, nothing changed.
    pub gate: Option<Gate>,
}

/// A question asked right before the installer runs: `None` to go on, or why not ([`Options::gate`]).
#[derive(Clone)]
pub struct Gate(std::sync::Arc<dyn Fn() -> Option<Failure> + Send + Sync>);

impl Gate {
    pub fn new(ask: impl Fn() -> Option<Failure> + Send + Sync + 'static) -> Gate {
        Gate(std::sync::Arc::new(ask))
    }
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Gate")
    }
}

impl Options {
    pub fn standard() -> Options {
        Options {
            package: current_package(),
            temp: std::env::temp_dir(),
            temp_often_redirected: crate::platform::staging::temp_often_redirected(),
            // A day: far past an install's own bounds (a 15 min download, 3 min for the installer).
            stale_after: Duration::from_secs(24 * 3600),
            download_limit: 512 * 1024 * 1024,
            download_time: Duration::from_secs(15 * 60),
            unpack: tarball::LIMITS,
            installer_time: Duration::from_secs(180),
            settle_time: Duration::from_secs(30),
            minor: None,
            gate: None,
        }
    }
}

/// Which directory the install's private directory is made in (`params.root`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Root {
    /// The system's temporary directory (`std::env::temp_dir`), there already and the OS's to clean.
    Temp,
    /// [`STAGING_DIR`] beside the `D` the install is for, made private by the install itself, which also removes what
    /// an interrupted install left in it.
    BesideData,
}

impl Root {
    const ALL: [Root; 2] = [Root::Temp, Root::BesideData];
}

/// Where the install's private directory is made: which kind of directory, and its path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagingRoot {
    pub root: Root,
    pub path: PathBuf,
}

/// The staging folder beside `D`, in `D`'s parent, named as the reset's control state beside the standard `D` is
/// (`ai.sidevoice.desktop-reset`).
pub const STAGING_DIR: &str = "ai.sidevoice.desktop-staging";

/// Where an install for the data directory `data` stages its download: the system's temporary directory `temp`, unless
/// this OS's is often redirected to a place other users can change (`temp_often_redirected`, from
/// [`crate::platform::staging`]); then [`STAGING_DIR`] beside `data`, in its parent.
///
/// `data` is the `D` the installer is run for, `$SIDEVOICE_DATA_DIR` when set (`DataDirs::from_env`), so the staging
/// folder's ancestry is exactly `D`'s: every directory above it is one above `D`, which the package's installer
/// requires the shared private-directory policy to accept anyway, and `D` itself is neither made nor written by the
/// app. Staging there asks nothing more of this computer. With the standard `D` on Windows that is
/// `%LOCALAPPDATA%\Sidevoice\ai.sidevoice.desktop-staging`, in the folder the app is installed in; with
/// `SIDEVOICE_DATA_DIR=E:\Profile`, `E:\ai.sidevoice.desktop-staging`. A `D` with no parent (a volume's root) stages
/// in the temporary directory.
pub fn staging_root(temp_often_redirected: bool, temp: PathBuf, data: &Path) -> StagingRoot {
    match data.parent().filter(|parent| temp_often_redirected && !parent.as_os_str().is_empty()) {
        Some(parent) => StagingRoot { root: Root::BesideData, path: parent.join(STAGING_DIR) },
        None => StagingRoot { root: Root::Temp, path: temp },
    }
}

/// The name of every install's private directory starts with this.
const SCRATCH_PREFIX: &str = "sidevoice-install-";

/// A fresh private directory for one install, removed with everything in it when dropped.
#[derive(Debug)]
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// A new private directory in `staging` (`Fs::create_private_dir`: 0700 on Unix, a protected owner-only DACL on
    /// Windows) under a fresh name (`sidevoice-install-` and six random characters, `tempfile`'s, taken only if nobody
    /// has it), then checked as the app checks its own directories: this user's, private, every directory above it
    /// safe from other users (sticky, as `/tmp`, is fine). The staging folder beside `D` ([`Root::BesideData`]) is
    /// first made private if missing (refused if it is there and not private), and what an interrupted install left in
    /// it more than `stale_after` ago removed ([`remove_stale`]).
    ///
    /// A failure is `install.unsafe` when the policy refuses a directory (`install.failed` / `install.disk` when the
    /// OS fails to make one), at step `download`, with `params` `{where, reason, os_error?, root}`
    /// ([`Failure::at`]). Its `message` is the English fallback the keys already had: no new sentence, the new facts
    /// are params the client translates.
    pub fn create(staging: &StagingRoot, stale_after: Duration) -> Result<Scratch, Failure> {
        let root = &staging.path;
        let rooted = |failure: Failure| failure.param("root", json!(staging.root));
        if !root.is_absolute() {
            let message = format!("The temporary directory {} is not absolute.", root.display());
            let failure = Failure::new("install.unsafe", Step::Download, message);
            return Err(rooted(failure.at(Where::StagingRoot, Reason::NotAbsolute, None)));
        }
        if staging.root == Root::BesideData {
            Files::ensure_private_dir(root).map_err(|e| {
                let what = root.display().to_string();
                rooted(if Reason::of(&e).is_refusal() {
                    Failure::unsafe_dir(&e, Step::Download, Where::StagingRoot, &what)
                } else {
                    Failure::io(&e, Step::Download, Where::StagingRoot, &what)
                })
            })?;
            remove_stale(root, stale_after);
        }
        let made = tempfile::Builder::new()
            .prefix(SCRATCH_PREFIX)
            .rand_bytes(6)
            .make_in(root, Files::create_private_dir)
            .and_then(|made| made.keep().map_err(|e| e.error))
            .map_err(|e| {
                let what = format!("Could not make a directory in {}", root.display());
                rooted(Failure::io(&e, Step::Download, Where::StagingRoot, &what))
            })?;
        // Removed again, by this name, if the check below refuses it.
        let mut scratch = Scratch { path: made.1 };
        let refused = |e: io::Error, path: &Path| {
            rooted(Failure::unsafe_dir(&e, Step::Download, Where::Scratch, &path.display().to_string()))
        };
        HeldDirectory::open(&scratch.path, Access::Private).map_err(|e| refused(e, &scratch.path))?;
        // Resolve only after the shared policy has accepted the original ancestry (e.g. macOS /var).
        scratch.path = scratch.path.canonicalize().map_err(|e| refused(e, &scratch.path))?;
        Ok(scratch)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Removes, as far as it can, the private directories an install that never finished (the app killed, the computer
/// turned off) left in the app's staging folder `root`, already made private: each directory named
/// [`SCRATCH_PREFIX`]… that last changed `stale_after` ago or more, one at a time, through the folder held open
/// (`Removal`: no link followed, nothing of another user's or that others can change, nothing else touched). One it
/// cannot remove stays, and the install goes on. Never in the system's temporary directory: that is the OS's to clean,
/// and what other users have there is not this app's to judge.
fn remove_stale(root: &Path, stale_after: Duration) {
    let Ok(held) = HeldDirectory::open(root, Access::Private) else { return };
    let Ok(entries) = std::fs::read_dir(root) else { return };
    let stale = |entry: &std::fs::DirEntry| {
        let named = entry.file_name().to_str().is_some_and(|name| name.starts_with(SCRATCH_PREFIX));
        // `DirEntry::metadata` does not follow a link: a link is not a directory here.
        let age = entry.metadata().ok().filter(|meta| meta.is_dir()).and_then(|meta| meta.modified().ok());
        named && age.and_then(|changed| changed.elapsed().ok()).is_some_and(|age| age >= stale_after)
    };
    let names: Vec<OsString> = entries.flatten().filter(stale).map(|entry| entry.file_name()).collect();
    for name in &names {
        let _ = held.remove_contents(&[name.as_os_str()]);
    }
}

/// Steps 1–4: the release resolved, downloaded, verified, unpacked and installed by its own installer. The
/// installer's answer (`{ok: true, action, version, …}`).
pub fn prepare(job: &Job, fetch: &dyn Fetch, data: &Path, options: &Options) -> Result<Value, Failure> {
    let package = options.package.as_deref().ok_or_else(|| {
        Failure::new("install.no-bundle", Step::Download, "Sidevoice has no package for this computer's system.")
    })?;
    job.enter(Step::Download)?;
    let (release, dist) = npm::resolve(fetch, package, options.minor)?;
    let staging = staging_root(options.temp_often_redirected, options.temp.clone(), data);
    let scratch = Scratch::create(&staging, options.stale_after)?;
    let archive = scratch.path().join("package.tgz");
    let digest = download(job, fetch, &dist, &archive, options)?;

    job.enter(Step::Verification)?;
    if digest != dist.sha512 {
        return Err(Failure::new(
            "install.checksum",
            Step::Verification,
            format!("{} {}'s tarball is not the one the registry describes.", release.package, release.version),
        ));
    }

    job.enter(Step::Staging)?;
    let file = std::fs::File::open(&archive)
        .map_err(|e| Failure::io(&e, Step::Staging, Where::Download, "Could not read the download"))?;
    tarball::extract(io::BufReader::new(file), scratch.path(), &installer_entry(), options.unpack, &|| {
        job.cancelled()
    })?;
    let installer = scratch.path().join(tarball::ROOT).join(bundled_installer());
    match std::fs::symlink_metadata(&installer) {
        Ok(meta) if meta.file_type().is_file() => {}
        _ => {
            return Err(Failure::new(
                "install.no-bundle",
                Step::Staging,
                format!("{} {} carries no {}.", release.package, release.version, bundled_installer()),
            ))
        }
    }
    // Unpacked as a program (`tarball::extract`); what runs is the path checked here.
    let installer = Files::trusted_program(&installer)
        .map_err(|e| Failure::io(&e, Step::Staging, Where::Installer, "The installer cannot be run"))?;
    if let Some(held) = options.gate.as_ref().and_then(|gate| (gate.0)()) {
        return Err(held);
    }
    job.commit()?;
    run_installer(job, &installer, data, options.installer_time)
}

/// The tarball into `file` (new and private, no link followed: `Fs::open_private`), its SHA-512 as it comes; within the size and time bounds, and stopping on a
/// cancel between chunks.
fn download(
    job: &Job,
    fetch: &dyn Fetch,
    dist: &npm::Dist,
    file: &Path,
    options: &Options,
) -> Result<[u8; 64], Failure> {
    let too_big = |bytes: u64| {
        Failure::new("install.authenticity", Step::Download, format!("The package is larger than {bytes} bytes."))
            .check("download-size")
    };
    let response = fetch.get(&dist.tarball)?;
    npm::expect_ok(&response, &dist.tarball)?;
    let total = response.length;
    if total.is_some_and(|total| total > options.download_limit) {
        return Err(too_big(options.download_limit));
    }
    job.bytes(0, total)?;
    let mut out = Files::open_private(file, Create::New)
        .map_err(|e| Failure::io(&e, Step::Download, Where::Download, "Could not save the download"))?;
    let mut body = response.body;
    let mut hasher = Sha512::new();
    let mut chunk = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let deadline = Instant::now() + options.download_time;
    loop {
        if job.cancelled() {
            return Err(Failure::cancelled(Step::Download));
        }
        if Instant::now() >= deadline {
            return Err(Failure::new(
                "install.network",
                Step::Download,
                format!("The download did not finish within {} s.", options.download_time.as_secs()),
            ));
        }
        let n = match body.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(Failure::new("install.network", Step::Download, format!("The download broke off: {e}")))
            }
        };
        done += n as u64;
        if done > options.download_limit {
            return Err(too_big(options.download_limit));
        }
        hasher.update(&chunk[..n]);
        out.write_all(&chunk[..n])
            .map_err(|e| Failure::io(&e, Step::Download, Where::Download, "Could not save the download"))?;
        job.bytes(done, total)?;
    }
    if total.is_some_and(|total| total != done) {
        return Err(Failure::new("install.network", Step::Download, format!("The download ended at {done} bytes.")));
    }
    Ok(hasher.finalize().into())
}

/// Its error output's last lines, for `log_tail`: at most 20, each at most 400 characters.
fn log_tail(stderr: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<String> = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .map(|line| line.chars().filter(|c| !c.is_control()).take(400).collect())
        .collect();
    lines[lines.len().saturating_sub(20)..].to_vec()
}

/// A file as it stands: which file `path` names (the identity of a handle opened on it without following a link,
/// `Fs::open_read` and `Fs::identity`) and when it was last written. Another reading differs once the file was written,
/// or another took its name (a rename over it). `None` when there is nothing there, a link, or it cannot be told.
fn stamp(path: &Path) -> Option<(FileId, SystemTime)> {
    let file = Files::open_read(path).ok()?;
    Some((Files::identity(&file).ok()?, file.metadata().ok()?.modified().ok()?))
}

/// `installer install --json`, with `SIDEVOICE_DATA_DIR` = `data`, as the app runs the CLI (no shell, a process group
/// of its own, killed past `timeout`). `service-start` once `install.json` changes.
fn run_installer(job: &Job, installer: &Path, data: &Path, timeout: Duration) -> Result<Value, Failure> {
    let program = installer.to_str().ok_or_else(|| {
        Failure::new("install.unsafe", Step::Staging, "The temporary directory's path is not UTF-8.").at(
            Where::Installer,
            Reason::NotUtf8,
            None,
        )
    })?;
    let record = data.join("install.json");
    let before = stamp(&record);
    let cli = Cli { prefix: vec![program.to_string()], data: data.to_path_buf() };
    let mut tick = || {
        if job.step() == Step::Staging && stamp(&record) != before {
            let _ = job.enter(Step::ServiceStart);
        }
    };
    let ran = cli
        .exec(&["install", "--json"], timeout, &mut tick)
        .map_err(|refusal| Failure::new("launch.failed", Step::Staging, refusal.message))?;
    tick();
    let step = job.step();
    let tail = log_tail(&ran.stderr);
    let with_tail = |mut failure: Failure| {
        failure.log_tail = (!tail.is_empty()).then(|| tail.clone());
        failure
    };
    let Some(status) = ran.status else {
        let message = format!("The installer did not finish within {} s.", timeout.as_secs());
        return Err(with_tail(Failure::new("cli.timeout", step, message)));
    };
    let answer: Option<Value> = serde_json::from_slice(&ran.stdout).ok().filter(Value::is_object);
    if let Some(answer) = &answer {
        if answer.get("ok") == Some(&Value::Bool(true)) && status.success() {
            return Ok(answer.clone());
        }
    }
    Err(with_tail(installer_failure(answer.as_ref(), status, step)))
}

/// The installer's `{ok: false, error: {key, params, message}}` as the page gets it: under the key sidevoice-web knows
/// where one means the same — `install.rolled-back` → `install.rollback`, `install.verify-failed` →
/// `service.start.failed`, `core.package-missing` → `install.no-bundle`, `core.package-mismatch` →
/// `install.authenticity` (`manifest`), `service.busy` → [`BUSY`], `connector.failed` → `install.failed` — else under its
/// own; its params and message as they are. No answer at all is `install.failed`.
pub fn installer_failure(answer: Option<&Value>, status: ExitStatus, step: Step) -> Failure {
    let error = answer.and_then(|a| a.get("error")).filter(|e| e.is_object());
    let Some(key) = error.and_then(|e| e.get("key")).and_then(Value::as_str) else {
        return Failure::new("install.failed", step, format!("The installer exited {status} without an answer."));
    };
    let message = error.and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or(key).to_string();
    let (key, check, step) = match key {
        "install.rolled-back" => ("install.rollback", None, Step::ServiceStart),
        "install.rollback-failed" => ("install.rollback-failed", None, Step::ServiceStart),
        "install.verify-failed" => ("service.start.failed", None, Step::ServiceStart),
        "core.package-missing" => ("install.no-bundle", None, step),
        "core.package-mismatch" => ("install.authenticity", Some("manifest"), step),
        "service.busy" => (BUSY, None, step),
        "connector.failed" => ("install.failed", None, step),
        other => (other, None, step),
    };
    let mut failure = Failure::new(key, step, message);
    failure.params = error
        .and_then(|e| e.get("params"))
        .filter(|p| p.is_object() && p.as_object().is_some_and(|o| !o.is_empty()))
        .cloned();
    match check {
        Some(check) => failure.check(check),
        None => failure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_report_has_the_key_the_step_and_the_known_params_and_no_free_text() {
        let mut failure = Failure::new("install.failed", Step::Staging, "free text: token=sensitive");
        failure.params = Some(json!({"check": "manifest", "detail": "unsafe private directory /tmp/x"}));
        failure.log_tail = Some(vec!["connector: unsafe private directory /tmp/x".into()]);
        let report = failure.log_report();
        assert_eq!(report, "install.failed at step staging (check=manifest) [1 param(s) not logged]");
    }

    #[test]
    fn param_values_that_are_not_their_grammar_are_counted_not_logged() {
        let mut failure = Failure::new("install.failed", Step::Staging, "m");
        failure.params = Some(json!({
            "found": "/home/alice/private", "expected": "hunter2", "version": "https://u:p@h/x?token=s",
            "status": "secret", "check": "C:\\Users\\a",
        }));
        assert_eq!(failure.log_report(), "install.failed at step staging [5 param(s) not logged]");
        let long = format!("1.2.3-{}+abcdef123456", "x".repeat(56));
        failure.params = Some(
            json!({"version": "1.2", "expected": "1.2.3_dev", "found": long, "status": 404, "check": "archive-link"}),
        );
        let report = failure.log_report();
        assert!(
            report.contains("version=1.2")
                && report.contains("expected=1.2.3_dev")
                && report.contains(&long)
                && report.contains("status=404"),
            "{report}"
        );
    }

    #[test]
    fn a_job_id_is_logged_only_if_it_is_shaped_like_one() {
        assert_eq!(loggable_job("3f2a-9c_1"), "3f2a-9c_1");
        assert_eq!(loggable_job("https://example.invalid/?token=secret"), "[invalid job]");
        assert_eq!(loggable_job("/home/alice/private"), "[invalid job]");
    }

    #[test]
    fn a_key_that_is_not_shaped_like_one_is_not_logged() {
        let failure = Failure::new("https://x/?token=sensitive", Step::Staging, "m");
        assert!(!failure.log_report().contains("sensitive"));
    }

    #[test]
    fn a_stamp_changes_when_another_file_takes_the_name_and_not_otherwise() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("install.json");
        assert_eq!(stamp(&path), None);
        std::fs::write(&path, b"{}").unwrap();
        let before = stamp(&path);
        assert!(before.is_some());
        assert_eq!(stamp(&path), before);
        let other = tmp.path().join("other.json");
        std::fs::write(&other, b"{}").unwrap();
        std::fs::rename(&other, &path).unwrap();
        assert_ne!(stamp(&path), before);
    }

    #[test]
    fn once_the_installer_runs_a_cancel_is_refused() {
        let job = Job::new("j");
        assert!(job.progress().cancellable);
        job.enter(Step::Staging).unwrap();
        job.commit().unwrap();
        assert!(!job.cancel(), "too late");
        assert!(job.enter(Step::ServiceStart).is_ok(), "and nothing was cancelled");
        let early = Job::new("k");
        assert!(early.cancel());
        assert!(early.cancel(), "asking again says the same");
        assert_eq!(early.commit().unwrap_err().key, CANCELLED, "a cancel before the switch wins");
    }

    /// The real registry, over the network: the latest release resolved, downloaded, verified and unpacked as an install
    /// would, stopping short of running its installer. By hand: `cargo test -p sidevoice-local-host --features fetch
    /// -- --ignored the_real_registry`.
    #[cfg(feature = "fetch")]
    #[test]
    #[ignore = "network"]
    fn the_real_registry_resolves_downloads_and_unpacks() {
        let tmp = tempfile::tempdir().unwrap();
        let options = Options { temp: tmp.path().into(), temp_often_redirected: false, ..Options::standard() };
        let options = Options { package: current_package(), ..options };
        let fetch = npm::Https::new().unwrap();
        let job = Job::new("real");
        let (release, dist) = npm::resolve(&fetch, options.package.as_deref().unwrap(), options.minor).unwrap();
        let staging = StagingRoot { root: Root::Temp, path: options.temp.clone() };
        let scratch = Scratch::create(&staging, options.stale_after).unwrap();
        let archive = scratch.path().join("package.tgz");
        let digest = download(&job, &fetch, &dist, &archive, &options).unwrap();
        assert_eq!(digest, dist.sha512, "{} {}", release.package, release.version);
        let file = std::fs::File::open(&archive).unwrap();
        tarball::extract(io::BufReader::new(file), scratch.path(), &installer_entry(), options.unpack, &|| false)
            .unwrap();
        let installer = scratch.path().join(tarball::ROOT).join(bundled_installer());
        assert!(Files::trusted_program(&installer).is_ok(), "executable");
        assert!(scratch.path().join("package/connector.json").is_file());
        let progress = job.progress();
        assert_eq!(progress.done, progress.total, "{progress:?}");
    }

    #[test]
    fn failures_serialise_as_the_page_reads_them() {
        let failure = Failure::new("install.authenticity", Step::Staging, "m").check("archive-link");
        assert_eq!(
            json!(failure),
            json!({"key": "install.authenticity", "step": "staging", "params": {"check": "archive-link"}, "message": "m"})
        );
        assert_eq!(json!(Failure::new(BUSY, None, "m")), json!({"key": "install.busy", "message": "m"}));
        let steps: Vec<Value> = [Step::Download, Step::Verification, Step::Staging, Step::ServiceStart, Step::Pairing]
            .iter()
            .map(|s| json!(s))
            .collect();
        assert_eq!(steps, ["download", "verification", "staging", "service-start", "pairing"]);
        let progress = Progress { step: Step::Download, done: Some(5), total: None, cancellable: true };
        assert_eq!(json!(progress), json!({"step": "download", "done": 5, "total": null, "cancellable": true}));
        let full = [io::Error::from(io::ErrorKind::StorageFull), io::Error::from(io::ErrorKind::QuotaExceeded)];
        for error in full {
            let disk = Failure::io(&error, Step::Staging, Where::Package, "w");
            assert_eq!(disk.key, "install.disk");
            assert_eq!(disk.step, Some(Step::Staging));
            assert_eq!(disk.message, format!("w: {error}"));
        }
        let other = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
        let failure = Failure::io(&other, Step::Download, Where::Download, "save");
        assert_eq!(failure.key, "install.failed");
        assert_eq!(failure.step, Some(Step::Download));
        assert_eq!(failure.message, format!("save: {other}"));
        assert_eq!(failure.params, Some(json!({"where": "download", "reason": "refused"})));
        let denied = io::Error::new(io::ErrorKind::PermissionDenied, "C:\\Temp: Access is denied. (os error 5)");
        let failure = Failure::io(&denied, Step::Staging, Where::Package, "write");
        assert_eq!(failure.params, Some(json!({"where": "package", "reason": "access-denied", "os_error": 5})));
        // Beside what is there already.
        let both = Failure::new("install.authenticity", Step::Staging, "m").check("archive-path").at(
            Where::Package,
            Reason::Os,
            Some(-3),
        );
        assert_eq!(
            both.params,
            Some(json!({"check": "archive-path", "where": "package", "reason": "os", "os_error": -3}))
        );
    }

    /// The shared policy's refusals as the pinned `sidevoice-platform` words them (`fs.rs`, `fs/unix.rs`,
    /// `fs/windows.rs`), and OS errors by kind; a new wording reads as `refused`, never as another reason.
    #[test]
    fn a_reason_is_read_from_the_policys_wording_and_the_errors_kind() {
        use io::ErrorKind::*;
        let cases = [
            (PermissionDenied, r"unsafe directory D:\Temp: others can replace what is in it", Reason::OthersCanReplace),
            (PermissionDenied, r"unsafe directory C:\Users\x: owned by another user", Reason::OwnedByOther),
            (PermissionDenied, "unsafe private directory /x/y: not this user's", Reason::OwnedByOther),
            (PermissionDenied, "unsafe private directory /x/y: open to others", Reason::OpenToOthers),
            (
                PermissionDenied,
                "unsafe private directory /x/y: unsafe ancestor owner or permissions",
                Reason::UnsafeAncestor,
            ),
            (
                PermissionDenied,
                r"unsafe directory C:\T: not an ordinary directory; choose an unredirected data directory",
                Reason::Link,
            ),
            (PermissionDenied, r"unsafe link C:\T: others can change it", Reason::Link),
            (PermissionDenied, "unsafe directory /tmp/l: a link of another user's", Reason::Link),
            (
                PermissionDenied,
                r"unsafe private directory Z:\: a network or shared drive; the directories behind it cannot be checked",
                Reason::Network,
            ),
            (PermissionDenied, "unsafe private directory /x: something new", Reason::Refused),
            (PermissionDenied, r"unsafe private directory C:\x: Access is denied. (os error 5)", Reason::AccessDenied),
            (InvalidInput, "relative is not an absolute path", Reason::NotAbsolute),
            (NotFound, "gone", Reason::NotFound),
            (AlreadyExists, "there", Reason::AlreadyExists),
            (StorageFull, "full", Reason::DiskFull),
            (QuotaExceeded, "quota", Reason::DiskFull),
            (ReadOnlyFilesystem, "ro", Reason::ReadOnly),
            (InvalidData, "odd", Reason::Os),
        ];
        for (kind, text, reason) in cases {
            assert_eq!(Reason::of(&io::Error::new(kind, text)), reason, "{text}");
        }
        for reason in Reason::ALL {
            let refusal = matches!(json!(reason).as_str(), Some(name) if !["not-found", "access-denied", "already-exists", "disk-full", "read-only", "os"].contains(&name));
            assert_eq!(reason.is_refusal(), refusal, "{reason:?}");
        }
    }

    #[test]
    fn the_os_code_is_the_raw_one_else_the_last_in_the_text() {
        assert_eq!(os_code(&io::Error::from_raw_os_error(2)), Some(2));
        let wrapped =
            io::Error::new(io::ErrorKind::PermissionDenied, r"C:\a (os error 9): Access is denied. (os error 5)");
        assert_eq!(os_code(&wrapped), Some(5));
        assert_eq!(os_code(&io::Error::other("no code (os error x)")), None);
        assert_eq!(os_code(&io::Error::other("no code")), None);
    }

    #[test]
    fn where_reason_root_and_os_error_are_logged_by_their_grammar_only() {
        let mut failure = Failure::new("install.unsafe", Step::Download, r"C:\Temp\sidevoice-install-x: refused");
        failure = failure.at(Where::Scratch, Reason::OthersCanReplace, Some(5)).param("root", json!(Root::Temp));
        assert_eq!(
            failure.log_report(),
            "install.unsafe at step download (os_error=5, reason=others-can-replace, root=temp, where=scratch)"
        );
        failure.params =
            Some(json!({"where": r"C:\Temp", "reason": "Others Can Replace", "root": "/tmp", "os_error": "5"}));
        assert_eq!(failure.log_report(), "install.unsafe at step download [4 param(s) not logged]");
        failure.params = Some(json!({"os_error": 1.5}));
        assert_eq!(failure.log_report(), "install.unsafe at step download [1 param(s) not logged]");
        for name in Where::ALL.iter().map(|w| json!(w)).chain(Reason::ALL.iter().map(|r| json!(r))) {
            let name = name.as_str().unwrap().to_string();
            assert!(name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{name}");
        }
    }

    /// Where an install stages, off any particular OS: beside the `D` it is for where the temporary directory is often
    /// redirected (Windows), whether that `D` is the standard one or `SIDEVOICE_DATA_DIR`'s; the temporary directory
    /// otherwise, or for a `D` with no parent.
    #[test]
    fn the_staging_root_is_beside_the_configured_data_directory_where_the_temporary_directory_is_often_redirected() {
        let temp = PathBuf::from("/system-temp");
        let standard = PathBuf::from("/home/a/AppData/Local/Sidevoice/Data");
        let beside = staging_root(true, temp.clone(), &standard);
        assert_eq!(beside, StagingRoot { root: Root::BesideData, path: standard.parent().unwrap().join(STAGING_DIR) });
        assert!(!beside.path.starts_with(&standard), "beside D, never in it");
        // `SIDEVOICE_DATA_DIR` elsewhere (another drive, a profile): beside that one, never the standard place.
        let configured = PathBuf::from("/profiles/E/SidevoiceProfile");
        let beside = staging_root(true, temp.clone(), &configured);
        assert_eq!(
            beside,
            StagingRoot { root: Root::BesideData, path: PathBuf::from("/profiles/E").join(STAGING_DIR) }
        );
        assert_eq!(beside.path.parent(), configured.parent(), "the same ancestry as D");
        assert_eq!(
            staging_root(false, temp.clone(), &configured),
            StagingRoot { root: Root::Temp, path: temp.clone() }
        );
        for rootless in [PathBuf::from("/"), PathBuf::from("relative")] {
            assert_eq!(staging_root(true, temp.clone(), &rootless).root, Root::Temp, "{rootless:?}");
        }
        assert_eq!(json!(Root::BesideData), "beside-data");
        // As the app builds it on this computer.
        let options = Options::standard();
        assert_eq!(options.temp_often_redirected, crate::platform::staging::temp_often_redirected());
        assert_eq!(options.temp, std::env::temp_dir());
    }
}
