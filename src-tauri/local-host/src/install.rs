//! Preparing this computer from the app, without Node (docs/LOCAL_HOST.md → Install): what `npx sidevoice@latest
//! install` does, with the npm package fetched directly.
//!
//! 1. `download`: the latest release and this computer's platform package, from the registry ([`npm`]), and its
//!    tarball into a fresh private directory ([`Scratch`]), its bytes counted as they come. Cancellable.
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
//! through ([`installer_failure`]).

use crate::cli::Cli;
use crate::npm::{self, Fetch};
use crate::platform::target::current_package;
use crate::tarball;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha512};
use sidevoice_platform::fs::{Access, Create, Directory, FileId, Files, Fs, HeldDirectory};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

/// What a second install is refused with while one runs (and what the installer's own lock, `service.busy`, becomes).
pub const BUSY: &str = "install.busy";
/// What an install the person cancelled rejects with.
pub const CANCELLED: &str = "install.cancelled";
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
/// (`params`: `check` for `install.authenticity`; the installer's own), the installer's last lines of error output,
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
    pub fn check(mut self, check: &str) -> Failure {
        let mut params = self.params.take().filter(Value::is_object).unwrap_or_else(|| json!({}));
        params["check"] = json!(check);
        self.params = Some(params);
        self
    }

    pub fn cancelled(step: Step) -> Failure {
        Failure::new(CANCELLED, step, "The install was cancelled.")
    }

    /// A local file could not be written: a full disk is `install.disk`, anything else `install.failed`.
    pub fn io(e: &io::Error, step: Step, what: &str) -> Failure {
        let full = matches!(e.kind(), io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded);
        Failure::new(if full { "install.disk" } else { "install.failed" }, step, format!("{what}: {e}"))
    }
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
                    Value::Number(n) if name == "status" && n.is_u64() => logged.push(format!("{name}={n}")),
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
/// `+` and 12 hex digits on a collision): digits and dots first, the rest from that alphabet, at most 96.
fn is_version(text: &str) -> bool {
    let mut parts = text.splitn(3, '.');
    let digits = |p: Option<&str>| p.is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    let rest = parts.clone().nth(2).unwrap_or("");
    text.len() <= 96
        && digits(parts.next())
        && digits(parts.next())
        && !rest.is_empty()
        && rest.starts_with(|c: char| c.is_ascii_digit())
        && rest.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '_' | '-'))
}

/// A digest: lowercase hex of a SHA-1 (npm `shasum`), SHA-256 or SHA-512 length.
fn is_digest(text: &str) -> bool {
    matches!(text.len(), 40 | 64 | 128) && text.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// A value is logged only if it has its param's grammar (never sanitized): `check` one of [`CHECKS`]; `version` a
/// release version; `expected` and `found` a version or a digest; `status` is logged only as a number (the
/// connector sends no word for it).
fn param_value_ok(name: &str, text: &str) -> bool {
    match name {
        "check" => CHECKS.contains(&text),
        "version" => is_version(text),
        "expected" | "found" => is_version(text) || is_digest(text),
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
    state: Mutex<JobState>,
}

#[derive(Debug)]
struct JobState {
    progress: Progress,
    cancelled: bool,
}

impl Job {
    pub fn new(id: &str) -> Job {
        let progress = Progress { step: Step::Download, done: None, total: None, cancellable: true };
        Job { id: id.to_string(), state: Mutex::new(JobState { progress, cancelled: false }) }
    }

    pub fn id(&self) -> &str {
        &self.id
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
    /// Where the private directory is made: the system's temporary directory.
    pub temp: PathBuf,
    /// The tarball's largest size, and how long its download may take in all.
    pub download_limit: u64,
    pub download_time: Duration,
    pub unpack: tarball::Limits,
    /// How long the installer may run before it is killed. Its own bounds: 30 s for the install lock, 30 s for the
    /// staged binary's self-check, 60 s to verify the service, 60 s more to verify a rollback.
    pub installer_time: Duration,
    /// How long the host waits for the new core to be reachable (paired) before it answers the state anyway.
    pub settle_time: Duration,
}

impl Options {
    pub fn standard() -> Options {
        Options {
            package: current_package(),
            temp: std::env::temp_dir(),
            download_limit: 512 * 1024 * 1024,
            download_time: Duration::from_secs(15 * 60),
            unpack: tarball::LIMITS,
            installer_time: Duration::from_secs(180),
            settle_time: Duration::from_secs(30),
        }
    }
}

/// A fresh private directory for one install, removed with everything in it when dropped.
#[derive(Debug)]
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// A new private directory below `root` (`Fs::create_private_dir`: 0700 on Unix) under a fresh name
    /// (`sidevoice-install-` and six random characters, `tempfile`'s, taken only if nobody has it), then checked as the
    /// app checks its own directories: this user's, private, every directory above it safe from other users (sticky,
    /// as `/tmp`, is fine).
    pub fn create(root: &Path) -> Result<Scratch, Failure> {
        let unsafe_root = |why: String| Failure::new("install.unsafe", Step::Download, why);
        if !root.is_absolute() {
            return Err(unsafe_root(format!("The temporary directory {} is not absolute.", root.display())));
        }
        let made = tempfile::Builder::new()
            .prefix("sidevoice-install-")
            .rand_bytes(6)
            .make_in(root, Files::create_private_dir)
            .and_then(|made| made.keep().map_err(|e| e.error))
            .map_err(|e| {
                Failure::io(&e, Step::Download, &format!("Could not make a directory in {}", root.display()))
            })?;
        // Removed again, by this name, if the check below refuses it.
        let mut scratch = Scratch { path: made.1 };
        HeldDirectory::open(&scratch.path, Access::Private)
            .map_err(|error| unsafe_root(format!("{}: {error}", scratch.path.display())))?;
        // Resolve only after the shared policy has accepted the original ancestry (e.g. macOS /var).
        scratch.path =
            scratch.path.canonicalize().map_err(|error| unsafe_root(format!("{}: {error}", scratch.path.display())))?;
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

/// Steps 1–4: the release resolved, downloaded, verified, unpacked and installed by its own installer. The
/// installer's answer (`{ok: true, action, version, …}`).
pub fn prepare(job: &Job, fetch: &dyn Fetch, data: &Path, options: &Options) -> Result<Value, Failure> {
    let package = options.package.as_deref().ok_or_else(|| {
        Failure::new("install.no-bundle", Step::Download, "Sidevoice has no package for this computer's system.")
    })?;
    job.enter(Step::Download)?;
    let (release, dist) = npm::resolve(fetch, package)?;
    let scratch = Scratch::create(&options.temp)?;
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
    let file =
        std::fs::File::open(&archive).map_err(|e| Failure::io(&e, Step::Staging, "Could not read the download"))?;
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
        .map_err(|e| Failure::io(&e, Step::Staging, "The installer cannot be run"))?;
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
        .map_err(|e| Failure::io(&e, Step::Download, "Could not save the download"))?;
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
        out.write_all(&chunk[..n]).map_err(|e| Failure::io(&e, Step::Download, "Could not save the download"))?;
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
    let program = installer
        .to_str()
        .ok_or_else(|| Failure::new("install.unsafe", Step::Staging, "The temporary directory's path is not UTF-8."))?;
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
        failure.params = Some(json!({"version": "1.2.3_dev", "found": long, "status": 404, "check": "archive-link"}));
        let report = failure.log_report();
        assert!(
            report.contains("version=1.2.3_dev") && report.contains(&long) && report.contains("status=404"),
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
        let options = Options { temp: tmp.path().into(), ..Options::standard() };
        let options = Options { package: current_package(), ..options };
        let fetch = npm::Https::new().unwrap();
        let job = Job::new("real");
        let (release, dist) = npm::resolve(&fetch, options.package.as_deref().unwrap()).unwrap();
        let scratch = Scratch::create(&options.temp).unwrap();
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
            let disk = Failure::io(&error, Step::Staging, "w");
            assert_eq!(disk.key, "install.disk");
            assert_eq!(disk.step, Some(Step::Staging));
            assert_eq!(disk.message, format!("w: {error}"));
        }
        let other = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
        let failure = Failure::io(&other, Step::Download, "save");
        assert_eq!(failure.key, "install.failed");
        assert_eq!(failure.step, Some(Step::Download));
        assert_eq!(failure.message, format!("save: {other}"));
    }
}
