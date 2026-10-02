//! The connector's CLI, as the app runs it (SEAMS §3): `command + [subcommand…, --json]`, where `command` is the
//! absolute argv prefix the install recorded in `D/install.json`. Never a shell, never a name looked up on a PATH,
//! always a deadline; one JSON object on stdout is the answer.
//!
//! [`Cli::installed`] is the one place that decides what runs the CLI. Until R4 that is the `npx`-installed
//! connector; R4 adds the executable the app ships here, and no caller changes.

use crate::checks::Check;
use crate::paths::DataDirs;
use crate::pin::ConnectorPin;
use crate::trusted;
use crate::Refusal;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: u64 = 1024 * 1024;
const RECORD_LIMIT: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    /// Absolute paths: the program, then what it needs before the subcommand (`[node, …/cli.mjs]`).
    pub prefix: Vec<String>,
    /// `D`, passed down as `SIDEVOICE_DATA_DIR` so the CLI and the app agree on it.
    pub data: std::path::PathBuf,
}

/// The structured progress event emitted by R4-b on stderr. Text is never parsed as progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressEvent {
    pub step: String,
    pub done: Option<u64>,
    pub total: Option<u64>,
}

/// Shared cancellation request. The CLI gets SIGINT and must acknowledge cancellation in its final JSON result;
/// if it has crossed its commit point it completes or rolls back instead.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn request(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_requested(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

impl Cli {
    /// The CLI the install recorded. `D`'s ancestry safe from other users, `D` itself this user's and not writable by
    /// others, `install.json` read through `D`'s checked descriptor (a regular file, not a link, this user's, not
    /// writable by others), and `command` a non-empty array of absolute paths, each a program or file only root or
    /// this user can change. No install, no CLI (`cli.unavailable`).
    pub fn installed(dirs: &DataDirs) -> Result<Cli, Refusal> {
        let record = dirs.install_record();
        let (dir, install) = open_install_record(dirs)?
            .ok_or_else(|| Refusal::new("cli.unavailable", "Sidevoice is not installed on this computer."))?;
        let prefix = command(&install).ok_or_else(|| {
            Refusal::new("install.unreadable", format!("{} names no command of absolute paths.", record.display()))
        })?;
        // What runs is exactly what was checked: each path resolved once, and `D` as its descriptor was checked.
        let mut verified = Vec::with_capacity(prefix.len());
        for path in &prefix {
            let resolved = checked(trusted::executable(std::path::Path::new(path)))?;
            let resolved = resolved.into_os_string().into_string().map_err(|_| {
                Refusal::new("install.unreadable", format!("{} names a path that is not UTF-8.", record.display()))
            })?;
            verified.push(resolved);
        }
        Ok(Cli { prefix: verified, data: dir.path().to_path_buf() })
    }

    /// Reads the trusted R1 install record without exposing its command to the page.
    pub fn install_record(dirs: &DataDirs) -> Result<Option<Value>, Refusal> {
        open_install_record(dirs).map(|record| record.map(|(_, value)| value))
    }

    /// Reads the selected connector release metadata through `install.json`'s recorded R root. The selection is
    /// captured from `R/current` as one validated `releases/<id>` link, then `release.json` is read through checked
    /// directory descriptors; the command and other install-record fields never supply version metadata.
    pub fn selected_release_record(dirs: &DataDirs) -> Result<Option<Value>, Refusal> {
        let Some((_, install)) = open_install_record(dirs)? else { return Ok(None) };
        let Some(root) = install.get("releases").and_then(Value::as_str).map(std::path::Path::new) else {
            return Ok(None);
        };
        if !root.is_absolute() || root.file_name() != Some(std::ffi::OsStr::new("sidevoice")) {
            return Err(Refusal::new(
                "install.unsafe",
                "The connector release directory is not a trusted Sidevoice path.",
            ));
        }
        let root = match trusted::Dir::open(root, 0o022, "install.unsafe") {
            Ok(root) => root,
            Err(crate::checks::Check::Missing) => return Ok(None),
            Err(crate::checks::Check::Unsafe(refusal)) => return Err(refusal),
        };
        let target = match root.read_link("current") {
            Ok(target) => target,
            Err(crate::checks::Check::Missing) => return Ok(None),
            Err(crate::checks::Check::Unsafe(refusal)) => return Err(refusal),
        };
        let Some(("releases", id)) = selected_release_target(&target) else {
            return Err(Refusal::new(
                "install.unsafe",
                "The connector current release link does not name a safe release.",
            ));
        };
        let releases = match root.open_child("releases", 0o022) {
            Ok(releases) => releases,
            Err(crate::checks::Check::Missing) => return Err(unreadable_selected_release()),
            Err(crate::checks::Check::Unsafe(refusal)) => return Err(refusal),
        };
        let release = match releases.open_child(id, 0o022) {
            Ok(release) => release,
            Err(crate::checks::Check::Missing) => return Err(unreadable_selected_release()),
            Err(crate::checks::Check::Unsafe(refusal)) => return Err(refusal),
        };
        let bytes = release.read("release.json", 0o022, RECORD_LIMIT).map_err(|check| match check {
            crate::checks::Check::Missing => unreadable_selected_release(),
            crate::checks::Check::Unsafe(refusal) => refusal,
        })?;
        let record: Value = serde_json::from_slice(&bytes).map_err(|error| {
            Refusal::new("install.unreadable", format!("The selected connector release metadata is not JSON: {error}"))
        })?;
        if record.get("id").and_then(Value::as_str) != Some(id) {
            return Err(unreadable_selected_release());
        }
        Ok(Some(record))
    }

    /// The app-bundled executable, after its checked resource path, executable mode, pinned bytes and R4-b embedded
    /// metadata are all verified. This path is independent of the installed R1 command and never consults `PATH`.
    pub fn bundled(resource: &std::path::Path, dirs: &DataDirs, pin: &ConnectorPin) -> Result<Cli, Refusal> {
        pin.validate_ready()?;
        let missing =
            || Refusal::new("install.executable-missing", "The app's bundled connector executable is missing.");
        let unsafe_resource =
            |why: &str| Refusal::new("install.unsafe", format!("The bundled connector resource is unsafe: {why}"));
        let meta = std::fs::symlink_metadata(resource).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                missing()
            } else {
                unsafe_resource(&e.to_string())
            }
        })?;
        if !meta.file_type().is_file() || meta.mode() & 0o111 == 0 || meta.mode() & 0o022 != 0 {
            return Err(unsafe_resource(&format!("mode {:o} is not a private executable file", meta.mode() & 0o7777)));
        }
        if meta.uid() != 0 && meta.uid() != unsafe { libc::geteuid() } {
            return Err(unsafe_resource("the resource has an unexpected owner"));
        }
        let path = std::fs::canonicalize(resource).map_err(|e| unsafe_resource(&e.to_string()))?;
        if path != resource {
            return Err(unsafe_resource("the resource path is not canonical"));
        }
        let expected_size = pin.executable_size.ok_or_else(missing)?;
        if meta.len() != expected_size {
            return Err(Refusal::new(
                "install.pin-mismatch",
                "The bundled connector size does not match its build pin.",
            ));
        }
        let actual_sha = sha256_file(resource).map_err(|e| unsafe_resource(&e.to_string()))?;
        if Some(actual_sha.as_str()) != pin.executable_sha256.as_deref() {
            return Err(Refusal::new(
                "install.pin-mismatch",
                "The bundled connector SHA-256 does not match its build pin.",
            ));
        }
        let cli = Cli { prefix: vec![path.to_string_lossy().into_owned()], data: dirs.data.clone() };
        let version = cli.run(&["--version", "--json"], Duration::from_secs(10))?;
        let metadata = cli.run(&["metadata", "--json"], Duration::from_secs(10))?;
        pin.verify_metadata(&version, &metadata)?;
        Ok(cli)
    }

    /// Runs `prefix + args` and waits at most `timeout` — for the process to exit **and** for both its output streams
    /// to close: its JSON answer when it says `ok`, else its `{key, message}`. Once an R4 install reports `commit` or
    /// `rollback`, the deadline stops applying so the connector can report the completed or restored transaction.
    ///
    /// The run is a process group of its own. Whatever is left in it once the program exits (a child still holding
    /// the pipes) is killed, and on a timeout before a transaction is finalizing the whole group is. What the program
    /// hands to the service manager
    /// (`service start` → launchd) or starts detached in a session of its own is not in the group, and stays.
    pub fn run(&self, args: &[&str], timeout: Duration) -> Result<Value, Refusal> {
        self.run_inner(args, timeout, None, Duration::from_secs(15), |_| {})
    }

    /// Runs an installer with the R4-b JSON-lines progress adapter. Both pipes are drained concurrently and output,
    /// including unfinished progress records, remains bounded. Cancellation is a request to the connector, not a
    /// claim that the transaction was cancelled; only its final keyed answer decides that.
    pub fn run_with_progress(
        &self,
        args: &[&str],
        timeout: Duration,
        cancel: &CancelToken,
        on_progress: impl FnMut(ProgressEvent),
    ) -> Result<Value, Refusal> {
        self.run_inner(args, timeout, Some(cancel), Duration::from_secs(15), on_progress)
    }

    fn run_inner(
        &self,
        args: &[&str],
        timeout: Duration,
        cancel: Option<&CancelToken>,
        cancel_grace: Duration,
        mut on_progress: impl FnMut(ProgressEvent),
    ) -> Result<Value, Refusal> {
        let failed = || Refusal::new("cli.failed", "The connector did not return a valid result.");
        let deadline = Instant::now() + timeout;
        let mut command = Command::new(&self.prefix[0]);
        command
            .args(&self.prefix[1..])
            .args(args)
            .env("SIDEVOICE_DATA_DIR", &self.data)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        // A program being written at that instant (an update, or any process that forked while holding it open for
        // writing) cannot be run yet: ETXTBSY, tried again shortly.
        let mut tries = 0;
        let mut child = loop {
            match command.spawn() {
                Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && tries < 5 => {
                    tries += 1;
                    thread::sleep(Duration::from_millis(50));
                }
                spawned => {
                    break spawned.map_err(|_| Refusal::new("cli.failed", "The connector could not be started."))?
                }
            }
        };
        let group = child.id() as libc::pid_t;
        let kill_group = || {
            // SAFETY: signals the process group this run created; ESRCH (nobody left) is fine.
            unsafe { libc::killpg(group, libc::SIGKILL) };
        };
        let mut streams =
            [Stream::new(child.stdout.take().map(OwnedFd::from)), Stream::new(child.stderr.take().map(OwnedFd::from))];
        let mut status = None;
        let mut cancel_sent = false;
        let cancel_deadline = timeout.min(cancel_grace);
        let mut cancel_at = None;
        let mut transaction_finalizing = false;
        loop {
            if !cancel_sent && cancel.is_some_and(CancelToken::is_requested) {
                // SAFETY: this is the process group created for this CLI invocation.
                unsafe { libc::killpg(group, libc::SIGINT) };
                cancel_sent = true;
                cancel_at = Some(Instant::now());
            }
            if cancel_at.is_some_and(|at| at.elapsed() >= cancel_deadline)
                && status.is_none()
                && !transaction_finalizing
            {
                kill_group();
            }
            if status.is_none() {
                status = child.try_wait().ok().flatten();
                if status.is_some() {
                    // The program answered and left: anything of its own still running only holds the pipes open.
                    kill_group();
                }
            }
            let open = streams.iter().any(Stream::is_open);
            if status.is_some() && !open {
                break;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() && !transaction_finalizing {
                break;
            }
            let poll_for = if left.is_zero() { Duration::from_millis(50) } else { left.min(Duration::from_millis(50)) };
            drain(&mut streams, poll_for, &mut |event| {
                if matches!(event.step.as_str(), "commit" | "rollback") {
                    transaction_finalizing = true;
                }
                on_progress(event);
            });
        }
        let Some(status) = status else {
            kill_group();
            let _ = child.kill();
            let _ = child.wait();
            return Err(Refusal::new(
                "cli.timeout",
                format!("`{}` did not answer within {} s.", args.join(" "), timeout.as_secs()),
            ));
        };
        let [stdout, _stderr] = streams.map(|s| s.bytes);
        let answer: Value = serde_json::from_slice(&stdout).ok().filter(Value::is_object).ok_or_else(failed)?;
        if answer.get("ok") == Some(&Value::Bool(true)) && status.success() {
            return Ok(answer);
        }
        Err(refusal_of(&answer).unwrap_or_else(failed))
    }
}

fn sha256_file(path: &std::path::Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// One of the program's output pipes, read without blocking past the deadline.
struct Stream {
    fd: Option<OwnedFd>,
    bytes: Vec<u8>,
    record: Vec<u8>,
    skipping_record: bool,
}

impl Stream {
    fn new(fd: Option<OwnedFd>) -> Self {
        Stream { fd, bytes: Vec::new(), record: Vec::new(), skipping_record: false }
    }

    fn is_open(&self) -> bool {
        self.fd.is_some()
    }
}

/// Waits at most `wait` for any open stream to have something, and reads what there is. End of file, or an error,
/// closes the stream; beyond [`OUTPUT_LIMIT`] bytes are read and dropped.
fn drain(streams: &mut [Stream], wait: Duration, on_progress: &mut impl FnMut(ProgressEvent)) {
    let mut polled: Vec<libc::pollfd> = streams
        .iter()
        .filter_map(|s| s.fd.as_ref())
        .map(|fd| libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 })
        .collect();
    let millis = wait.as_millis().clamp(1, 1000) as libc::c_int;
    // SAFETY: `polled` is a valid array of its length; with none, poll only sleeps.
    let ready = unsafe { libc::poll(polled.as_mut_ptr(), polled.len() as libc::nfds_t, millis) };
    if ready <= 0 {
        return;
    }
    let mut events = polled.iter().map(|p| p.revents);
    for (index, stream) in streams.iter_mut().enumerate().filter(|(_, s)| s.fd.is_some()) {
        if events.next().unwrap_or(0) == 0 {
            continue;
        }
        let fd = stream.fd.as_ref().expect("open").as_raw_fd();
        let mut chunk = [0u8; 16 * 1024];
        // SAFETY: `chunk` is writable for its length; the fd is open. poll said it would not block.
        let n = unsafe { libc::read(fd, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) };
        if n <= 0 {
            stream.fd = None;
        } else if (stream.bytes.len() as u64) < OUTPUT_LIMIT {
            stream.bytes.extend_from_slice(&chunk[..n as usize]);
        }
        if index == 1 && n > 0 {
            for byte in &chunk[..n as usize] {
                if *byte == b'\n' {
                    if !stream.skipping_record {
                        if let Some(event) = progress_record(&stream.record) {
                            on_progress(event);
                        }
                    }
                    stream.record.clear();
                    stream.skipping_record = false;
                } else if !stream.skipping_record {
                    if stream.record.len() < RECORD_LIMIT as usize {
                        stream.record.push(*byte);
                    } else {
                        stream.record.clear();
                        stream.skipping_record = true;
                    }
                }
            }
        }
    }
}

fn progress_record(bytes: &[u8]) -> Option<ProgressEvent> {
    let record: Value = serde_json::from_slice(bytes).ok()?;
    if record.get("type")?.as_str()? != "progress" {
        return None;
    }
    let step = record.get("step")?.as_str()?;
    if !matches!(
        step,
        "download"
            | "verify"
            | "stage"
            | "service-start"
            | "wait-calls"
            | "wait-lock"
            | "commit"
            | "pairing"
            | "rollback"
    ) {
        return None;
    }
    let number = |field: &str| match record.get(field) {
        None | Some(Value::Null) => Some(None),
        Some(value) => value.as_u64().map(Some),
    };
    let done = number("done")?;
    let total = number("total")?;
    if done.zip(total).is_some_and(|(done, total)| done > total) {
        return None;
    }
    Some(ProgressEvent { step: step.into(), done, total })
}

/// A check's outcome for the CLI: missing is "not installed".
fn checked<T>(result: Result<T, Check>) -> Result<T, Refusal> {
    result.map_err(|check| match check {
        Check::Missing => Refusal::new("cli.unavailable", "Sidevoice is not installed on this computer."),
        Check::Unsafe(refusal) => refusal,
    })
}

fn open_install_record(dirs: &DataDirs) -> Result<Option<(trusted::Dir, Value)>, Refusal> {
    let dir = match trusted::Dir::open(&dirs.data, 0o022, "install.unsafe") {
        Ok(dir) => dir,
        Err(Check::Missing) => return Ok(None),
        Err(Check::Unsafe(refusal)) => return Err(refusal),
    };
    let text = match dir.read("install.json", 0o022, RECORD_LIMIT) {
        Ok(text) => text,
        Err(Check::Missing) => return Ok(None),
        Err(Check::Unsafe(refusal)) => return Err(refusal),
    };
    let record = dirs.install_record();
    let install: Value = serde_json::from_slice(&text)
        .map_err(|e| Refusal::new("install.unreadable", format!("{} is not JSON: {e}", record.display())))?;
    Ok(Some((dir, install)))
}

fn selected_release_target(target: &std::path::Path) -> Option<(&str, &str)> {
    use std::path::Component;
    if target.is_absolute() {
        return None;
    }
    let mut components = target.components();
    let Component::Normal(parent) = components.next()? else { return None };
    let Component::Normal(id) = components.next()? else { return None };
    if components.next().is_some() || parent != "releases" {
        return None;
    }
    let id = id.to_str()?;
    let exact = target.to_str()? == format!("releases/{id}");
    (exact
        && !id.is_empty()
        && id != "."
        && id != ".."
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte)))
    .then_some(("releases", id))
}

fn unreadable_selected_release() -> Refusal {
    Refusal::new("install.unreadable", "The selected connector release metadata is missing or inconsistent.")
}

/// `install.json`'s `command`: every element an absolute path.
fn command(install: &Value) -> Option<Vec<String>> {
    let items = install.get("command")?.as_array()?;
    let prefix: Vec<String> = items.iter().map(|v| v.as_str().map(str::to_string)).collect::<Option<_>>()?;
    let absolute = |p: &String| p.starts_with('/') && !p.contains('\0');
    (!prefix.is_empty() && prefix.iter().all(absolute)).then_some(prefix)
}

/// `{ok: false, error: {key, params?: {check}}}`. Never forward connector prose, log tails or arbitrary parameters.
fn refusal_of(answer: &Value) -> Option<Refusal> {
    let error = answer.get("error")?;
    let key = error.get("key")?.as_str()?;
    let (safe_key, message) = match key {
        "install.network" => ("install.network", "The connector could not download the core."),
        "install.proxy" => ("install.proxy", "The connector could not reach the core through the network proxy."),
        "install.disk" => ("install.disk", "There is not enough disk space to install the core."),
        "install.checksum" => ("install.checksum", "The downloaded core did not match its pinned checksum."),
        "install.no-bundle" => ("install.no-bundle", "No compatible core bundle is available."),
        "install.authenticity" => ("install.authenticity", "The connector could not verify the core's authenticity."),
        "install.self-test" => ("install.self-test", "The installed core did not pass its self-test."),
        "install.rollback" => ("install.rollback", "The connector rolled back the installation after a failure."),
        "install.rollback-failed" => {
            ("install.rollback-failed", "The connector could not restore the previous installation.")
        }
        "install.rollback-registration" => (
            "install.rollback-registration",
            "The connector restored the previous installation but could not restore its service registration.",
        ),
        "install.incompatible" => ("install.incompatible", "The installed core is not compatible with this connector."),
        "service.not-loaded" => ("service.not-loaded", "The local service is not loaded."),
        "service.failed" => ("service.failed", "The local service did not start."),
        "service.start.failed" => ("service.start.failed", "The local service could not be started."),
        "launch.failed" => ("launch.failed", "The local service could not be launched."),
        "launch.exited" => ("launch.exited", "The local core exited unexpectedly."),
        "install.cancelled" => ("install.cancelled", "The install was cancelled before commit."),
        _ => return Some(Refusal::new("cli.failed", "The connector returned an unrecognized failure.")),
    };
    let check = error.get("params").and_then(|params| params.get("check")).and_then(Value::as_str);
    Some(match check {
        Some(check) => Refusal::new(safe_key, message).with_check(check),
        None => Refusal::new(safe_key, message),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A fake CLI: a shell script whose absolute path is the whole prefix. It prints `$SIDEVOICE_DATA_DIR` and its
    /// arguments into a JSON answer, or whatever the arguments ask for.
    fn fake(dir: &std::path::Path, body: &str) -> Cli {
        let script = dir.join("sidevoice");
        std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        Cli { prefix: vec![script.to_string_lossy().into_owned()], data: dir.to_path_buf() }
    }

    fn selected_release_fixture(kind: &str) -> Value {
        let (install_fixture, release_fixture) = match kind {
            "r1" => (
                include_str!("../test-fixtures/connector-install-r1.json"),
                include_str!("../test-fixtures/connector-release-r1.json"),
            ),
            "r4" => (
                include_str!("../test-fixtures/connector-install-r4.json"),
                include_str!("../test-fixtures/connector-release-r4.json"),
            ),
            _ => panic!("unknown connector fixture"),
        };
        let tmp = tempfile::Builder::new().prefix("svcli").tempdir_in("/tmp").unwrap();
        let dirs = DataDirs::new(tmp.path().join("data"));
        std::fs::create_dir(&dirs.data).unwrap();
        std::fs::set_permissions(&dirs.data, std::fs::Permissions::from_mode(0o700)).unwrap();

        let root = tmp.path().join("sidevoice");
        let releases = root.join("releases");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(&releases).unwrap();
        let release: Value = serde_json::from_str(release_fixture).unwrap();
        let id = release["id"].as_str().unwrap();
        let release_dir = releases.join(id);
        std::fs::create_dir(&release_dir).unwrap();
        std::fs::write(release_dir.join("release.json"), release_fixture).unwrap();
        std::os::unix::fs::symlink(format!("releases/{id}"), root.join("current")).unwrap();

        let mut install: Value = serde_json::from_str(install_fixture).unwrap();
        install["releases"] = Value::String(root.to_string_lossy().into_owned());
        std::fs::write(dirs.install_record(), serde_json::to_vec(&install).unwrap()).unwrap();
        std::fs::set_permissions(dirs.install_record(), std::fs::Permissions::from_mode(0o600)).unwrap();
        Cli::selected_release_record(&dirs).unwrap().unwrap()
    }

    #[test]
    fn r1_and_r4_install_records_resolve_metadata_from_the_selected_release() {
        let r1 = selected_release_fixture("r1");
        assert_eq!(r1["connector"], "0.5.0");
        assert_eq!(r1["channel"], "release");
        assert_eq!(r1["build_seq"], 14);
        assert_eq!(r1["core_build"], "0.1.0-macos-aarch64-2026-09-28");

        let r4 = selected_release_fixture("r4");
        assert_eq!(r4["connector"], "1.2.3");
        assert_eq!(r4["channel"], "nightly");
        assert_eq!(r4["build_seq"], 42);
        assert_eq!(r4["core_build"], "0.1.0-macos-aarch64-2026-10-02");
    }

    #[test]
    fn r1_and_r4_selected_releases_drive_update_order_without_downgrades() {
        use crate::versioning::{self, UpdateStatus};
        let pin = crate::pin::test_support::fixture_pin();

        let r1 = selected_release_fixture("r1");
        let r1_build = crate::pin::InstalledBuild::from_release_record(&r1, Some(1), None);
        assert_eq!(versioning::update_status(Some(&pin), Some(&r1_build)), UpdateStatus::Available);

        let r4 = selected_release_fixture("r4");
        let r4_build = crate::pin::InstalledBuild::from_release_record(&r4, Some(1), None);
        assert_eq!(versioning::update_status(Some(&pin), Some(&r4_build)), UpdateStatus::Current);

        let mut newer = r4.clone();
        newer["connector"] = Value::String("1.2.4".into());
        let newer_build = crate::pin::InstalledBuild::from_release_record(&newer, Some(1), None);
        assert_eq!(versioning::update_status(Some(&pin), Some(&newer_build)), UpdateStatus::NewerInstalled);

        let mut newer_core = r4;
        newer_core["connector"] = Value::String("1.3.0".into());
        newer_core["core"] = Value::String("0.2.0".into());
        let newer_core_build = crate::pin::InstalledBuild::from_release_record(&newer_core, Some(1), None);
        assert_eq!(versioning::update_status(Some(&pin), Some(&newer_core_build)), UpdateStatus::NewerInstalled);
    }

    #[test]
    fn selected_release_metadata_rejects_a_current_link_that_escapes_r() {
        let tmp = tempfile::Builder::new().prefix("svcli").tempdir_in("/tmp").unwrap();
        let dirs = DataDirs::new(tmp.path().join("data"));
        std::fs::create_dir(&dirs.data).unwrap();
        std::fs::set_permissions(&dirs.data, std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = tmp.path().join("sidevoice");
        std::fs::create_dir(&root).unwrap();
        let mut install: Value =
            serde_json::from_str(include_str!("../test-fixtures/connector-install-r1.json")).unwrap();
        install["releases"] = Value::String(root.to_string_lossy().into_owned());
        std::fs::write(dirs.install_record(), serde_json::to_vec(&install).unwrap()).unwrap();
        std::fs::set_permissions(dirs.install_record(), std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink("../../outside", root.join("current")).unwrap();
        assert_eq!(Cli::selected_release_record(&dirs).unwrap_err().key, "install.unsafe");
    }

    #[test]
    fn the_answer_is_the_json_on_stdout() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = fake(tmp.path(), r#"echo "{\"ok\":true,\"args\":\"$*\",\"data\":\"$SIDEVOICE_DATA_DIR\"}""#);
        let answer = cli.run(&["service", "start", "--json"], Duration::from_secs(5)).unwrap();
        assert_eq!(answer["args"], "service start --json");
        assert_eq!(answer["data"], tmp.path().to_string_lossy().as_ref());
    }

    #[test]
    fn a_refusal_carries_its_key() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = fake(
            tmp.path(),
            r#"echo '{"ok":false,"error":{"key":"service.failed","message":"token=secret"}}'; exit 1"#,
        );
        let refusal = cli.run(&["service", "start", "--json"], Duration::from_secs(5)).unwrap_err();
        assert_eq!(refusal, Refusal::new("service.failed", "The local service did not start."));
    }

    #[test]
    fn connector_authenticity_refusal_forwards_only_the_allowlisted_check() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = fake(
            tmp.path(),
            r#"echo '{"ok":false,"error":{"key":"install.authenticity","message":"secret token prose","params":{"check":"repository-id","secret":"ignored"},"log_tail":"ignored"}}'; exit 1"#,
        );
        let refusal = cli.run(&["install", "--json"], Duration::from_secs(5)).unwrap_err();
        assert_eq!(refusal.key, "install.authenticity");
        assert_eq!(refusal.message, "The connector could not verify the core's authenticity.");
        assert_eq!(refusal.params.as_ref().map(|params| params.check.as_str()), Some("repository-id"));
        let serialized = serde_json::to_string(&refusal).unwrap();
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("log_tail"));

        let unsafe_cli = fake(
            tmp.path(),
            r#"echo '{"ok":false,"error":{"key":"install.authenticity","message":"ignored","params":{"check":"/private/token"}}}'; exit 1"#,
        );
        let untrusted_check = unsafe_cli.run(&["install", "--json"], Duration::from_secs(5)).unwrap_err();
        assert_eq!(untrusted_check.params, None);

        let redirect_cli = fake(
            tmp.path(),
            r#"echo '{"ok":false,"error":{"key":"install.authenticity","message":"redirect prose","params":{"check":"redirect"}}}'; exit 1"#,
        );
        let redirect = redirect_cli.run(&["install", "--json"], Duration::from_secs(5)).unwrap_err();
        assert_eq!(redirect.key, "install.authenticity");
        assert_eq!(redirect.params.as_ref().map(|params| params.check.as_str()), Some("redirect"));
    }

    #[test]
    fn progress_jsonl_is_delivered_in_order_and_prose_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = fake(
            tmp.path(),
            r#"echo 'localized words are not progress' >&2
echo '{"type":"progress","step":"download","done":5,"total":12}' >&2
echo '{"type":"progress","step":"verify","done":null,"total":null}' >&2
echo '{"ok":true}'"#,
        );
        let mut events = Vec::new();
        let answer = cli
            .run_with_progress(&["install", "--json"], Duration::from_secs(5), &CancelToken::default(), |event| {
                events.push(event)
            })
            .unwrap();
        assert_eq!(answer["ok"], true);
        assert_eq!(
            events,
            [
                ProgressEvent { step: "download".into(), done: Some(5), total: Some(12) },
                ProgressEvent { step: "verify".into(), done: None, total: None },
            ]
        );
    }

    #[test]
    fn cancellation_does_not_kill_an_installer_after_commit_progress() {
        let tmp = tempfile::tempdir().unwrap();
        let signal_seen = tmp.path().join("cancel-signal-seen");
        let script = format!(
            r#"trap 'touch "{signal_seen}"' INT
echo '{{"type":"progress","step":"commit","done":null,"total":null}}' >&2
while [ ! -e "{signal_seen}" ]; do sleep 0.01; done
sleep 0.25
echo '{{"ok":true}}'"#,
            signal_seen = signal_seen.display()
        );
        let cli = fake(tmp.path(), &script);
        let cancel = CancelToken::default();
        let answer = cli
            .run_inner(
                &["install", "--json", "--progress=jsonl"],
                Duration::from_secs(3),
                Some(&cancel),
                Duration::from_millis(75),
                |event| {
                    if event.step == "commit" {
                        cancel.request();
                    }
                },
            )
            .unwrap();
        assert_eq!(answer["ok"], true);
        assert!(cancel.is_requested());
        assert!(signal_seen.exists(), "the installer received the requested SIGINT");
    }

    #[test]
    fn overall_timeout_does_not_kill_a_finalizing_install() {
        for step in ["commit", "rollback"] {
            let tmp = tempfile::tempdir().unwrap();
            let cli = fake(
                tmp.path(),
                &format!(
                    r#"echo '{{"type":"progress","step":"{step}","done":null,"total":null}}' >&2
sleep 0.25
echo '{{"ok":true}}'"#
                ),
            );
            let started = Instant::now();
            let answer = cli
                .run_inner(
                    &["install", "--json", "--progress=jsonl"],
                    Duration::from_millis(100),
                    None,
                    Duration::from_millis(20),
                    |_| {},
                )
                .expect("a finalizing transaction must finish and return its final result");
            assert_eq!(answer["ok"], true, "{step}");
            assert!(started.elapsed() >= Duration::from_millis(250), "{step}");
        }
    }

    #[test]
    fn connector_refusal_keys_are_preserved_without_forwarding_connector_prose() {
        for key in ["install.rollback-registration", "install.incompatible", "launch.exited"] {
            let refusal = refusal_of(&serde_json::json!({
                "ok": false,
                "error": { "key": key, "message": "untrusted connector detail", "params": { "log_tail": ["secret"] } }
            }))
            .unwrap();
            assert_eq!(refusal.key, key);
            assert!(!refusal.message.contains("untrusted"));
            assert!(!refusal.message.contains("secret"));
        }
    }

    #[test]
    fn bundled_constructor_checks_file_identity_mode_and_embedded_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let executable = tmp.path().join("sidevoice");
        let mut pin = crate::pin::test_support::fixture_pin();
        let manifest_sha = pin.core_manifest_sha256.as_deref().unwrap();
        let assets_json = serde_json::to_string(&pin.core_assets).unwrap();
        let script = r##"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo '{"ok":true,"version":"1.2.3","target":"macos-aarch64","channel":"nightly","connector_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","build_seq":42}'
elif [ "$1" = "metadata" ]; then
  echo '{"ok":true,"connector":{"version":"1.2.3","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","channel":"nightly","build_seq":42,"link_min":1,"link_max":1},"embedded_core":{"version":"0.1.0","manifest_sha256":"MANIFEST_SHA","assets":CORE_ASSETS,"api":1,"link":1},"protocols":{"metadata":"sidevoice-metadata-v1","progress":"sidevoice-progress-jsonl-v1"}}'
fi
"##;
        let script = script.replace("MANIFEST_SHA", manifest_sha).replace("CORE_ASSETS", &assets_json);
        std::fs::write(&executable, &script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        pin.executable_size = Some(script.len() as u64);
        pin.executable_sha256 = Some(sha256_file(&executable).unwrap());
        let cli = Cli::bundled(&executable, &DataDirs::new(tmp.path()), &pin).unwrap();
        assert_eq!(cli.prefix, [executable.to_string_lossy().to_string()]);

        pin.executable_sha256 = Some("0".repeat(64));
        assert_eq!(
            Cli::bundled(&executable, &DataDirs::new(tmp.path()), &pin).unwrap_err().key,
            "install.pin-mismatch"
        );
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(Cli::bundled(&executable, &DataDirs::new(tmp.path()), &pin).unwrap_err().key, "install.unsafe");
    }

    #[test]
    fn bundled_constructor_refuses_a_pending_production_pin_before_running_anything() {
        let tmp = tempfile::tempdir().unwrap();
        let executable = tmp.path().join("missing");
        let mut pin = crate::pin::test_support::fixture_pin();
        pin.status = "pending".into();
        assert_eq!(Cli::bundled(&executable, &DataDirs::new(tmp.path()), &pin).unwrap_err().key, "install.pin-invalid");
    }

    #[test]
    fn no_json_a_crash_or_a_hang_are_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = fake(tmp.path(), "echo boom >&2; exit 3");
        let refusal = cli.run(&["x"], Duration::from_secs(5)).unwrap_err();
        assert_eq!(refusal.key, "cli.failed");
        assert_eq!(refusal.message, "The connector did not return a valid result.");
        let cli = fake(tmp.path(), r#"echo '{"ok":true}'; exit 2"#);
        assert_eq!(cli.run(&["x"], Duration::from_secs(5)).unwrap_err().key, "cli.failed");
        let cli = fake(tmp.path(), "sleep 30");
        let started = Instant::now();
        assert_eq!(cli.run(&["x"], Duration::from_millis(300)).unwrap_err().key, "cli.timeout");
        assert!(started.elapsed() < Duration::from_secs(5));
        let missing = Cli { prefix: vec!["/nonexistent/sidevoice".into()], data: tmp.path().into() };
        assert_eq!(missing.run(&["x"], Duration::from_secs(1)).unwrap_err().key, "cli.failed");
    }

    /// Whether `pid` is still running (a zombie waiting for its reaper is not).
    fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 only asks whether the process exists.
        let exists = unsafe { libc::kill(pid, 0) } == 0;
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        exists && !stat.rsplit_once(')').is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'))
    }

    fn dead_soon(pid: i32) -> bool {
        (0..100).any(|_| {
            !alive(pid) || {
                thread::sleep(Duration::from_millis(20));
                false
            }
        })
    }

    fn pid_in(path: &std::path::Path) -> i32 {
        std::fs::read_to_string(path).unwrap().trim().parse().unwrap()
    }

    #[test]
    fn a_child_left_holding_the_pipes_neither_delays_the_answer_nor_survives() {
        let tmp = tempfile::tempdir().unwrap();
        let pid = tmp.path().join("pid");
        let cli = fake(tmp.path(), &format!("sleep 30 & echo $! > '{}'\necho '{{\"ok\":true}}'", pid.display()));
        let started = Instant::now();
        assert_eq!(cli.run(&["x"], Duration::from_secs(10)).unwrap()["ok"], true);
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        assert!(dead_soon(pid_in(&pid)), "the leftover child was killed");
    }

    #[test]
    fn a_timeout_kills_the_whole_run() {
        let tmp = tempfile::tempdir().unwrap();
        let pid = tmp.path().join("pid");
        let cli = fake(tmp.path(), &format!("sleep 30 & echo $! > '{}'\nsleep 30", pid.display()));
        let started = Instant::now();
        assert_eq!(cli.run(&["x"], Duration::from_millis(500)).unwrap_err().key, "cli.timeout");
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        assert!(dead_soon(pid_in(&pid)), "the child was killed with the program");
    }

    #[test]
    fn what_the_program_detaches_into_its_own_session_stays() {
        let tmp = tempfile::tempdir().unwrap();
        let pid = tmp.path().join("pid");
        // As the connector's detached spawn: a session of its own before the program goes on, its output elsewhere.
        let detach = format!(
            "perl -e 'use POSIX; POSIX::setsid(); open F, \">{p}\"; print F $$; close F; exec \"sleep\", \"30\"' </dev/null >/dev/null 2>&1 &\nwhile [ ! -s '{p}' ]; do sleep 0.05; done\necho '{{\"ok\":true}}'",
            p = pid.display()
        );
        let cli = fake(tmp.path(), &detach);
        assert_eq!(cli.run(&["service", "start", "--json"], Duration::from_secs(10)).unwrap()["ok"], true);
        let pid = pid_in(&pid);
        thread::sleep(Duration::from_millis(200));
        assert!(alive(pid), "a service the program started on its own stays");
        // SAFETY: the test's own process.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }

    #[test]
    fn what_runs_is_what_was_checked_even_if_a_link_is_retargeted() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let program = |name: &str| {
            let path = tmp.path().join(name);
            std::fs::write(
                &path,
                format!(
                    "#!/bin/sh\necho '{{\"ok\":true,\"which\":\"{name}\",\"data\":\"'\"$SIDEVOICE_DATA_DIR\"'\"}}'\n"
                ),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            path
        };
        let (genuine, replacement) = (program("genuine"), program("replacement"));
        let link = tmp.path().join("sidevoice");
        std::os::unix::fs::symlink(&genuine, &link).unwrap();
        let dirs = DataDirs::new(tmp.path());
        std::fs::write(dirs.install_record(), format!(r#"{{"command":["{}"]}}"#, link.display())).unwrap();
        std::fs::set_permissions(dirs.install_record(), std::fs::Permissions::from_mode(0o600)).unwrap();
        let cli = Cli::installed(&dirs).unwrap();
        // Between the check and the run, the link is pointed elsewhere.
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&replacement, &link).unwrap();
        let answer = cli.run(&["x"], Duration::from_secs(5)).unwrap();
        assert_eq!(answer["which"], "genuine", "the checked target runs, not the link's new one");
        let canonical_data = std::fs::canonicalize(tmp.path()).unwrap();
        assert_eq!(answer["data"], canonical_data.display().to_string(), "SIDEVOICE_DATA_DIR is the checked directory");
    }

    #[test]
    fn only_an_install_record_of_absolute_paths_names_the_cli() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let dirs = DataDirs::new(tmp.path());
        assert_eq!(Cli::installed(&dirs).unwrap_err().key, "cli.unavailable");
        let write = |text: &str, mode: u32| {
            std::fs::write(dirs.install_record(), text).unwrap();
            std::fs::set_permissions(dirs.install_record(), std::fs::Permissions::from_mode(mode)).unwrap();
        };
        let script = tmp.path().join("cli.mjs");
        std::fs::write(&script, b"").unwrap();
        let record = format!(r#"{{"command":["/bin/sh","{}"]}}"#, script.display());
        write(&record, 0o600);
        let cli = Cli::installed(&dirs).unwrap();
        let canonical = |p: &str| std::fs::canonicalize(p).unwrap().display().to_string();
        assert_eq!(cli.prefix, [canonical("/bin/sh"), canonical(&script.display().to_string())]);
        // What the command names must be root's or this user's and closed to others too.
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert_eq!(Cli::installed(&dirs).unwrap_err().key, "install.unsafe");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).unwrap();
        write(r#"{"command":["/nonexistent/node"]}"#, 0o600);
        assert_eq!(Cli::installed(&dirs).unwrap_err().key, "cli.unavailable");
        for bad in [r#"{"command":["node","cli.mjs"]}"#, r#"{"command":[]}"#, r#"{"command":"/bin/sh"}"#, "{}", "nope"]
        {
            write(bad, 0o600);
            let key = Cli::installed(&dirs).unwrap_err().key;
            assert_eq!(key, "install.unreadable", "{bad}");
        }
        write(r#"{"command":["/bin/sh"]}"#, 0o666);
        assert_eq!(Cli::installed(&dirs).unwrap_err().key, "install.unsafe");
        write(r#"{"command":["/bin/sh"]}"#, 0o600);
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(Cli::installed(&dirs).unwrap_err().key, "install.unsafe");
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}
