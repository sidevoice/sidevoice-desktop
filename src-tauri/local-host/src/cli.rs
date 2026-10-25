//! The connector's CLI, as the app runs it (SEAMS §3): `command + [subcommand…, --json]`, where `command` is the
//! absolute argv prefix the install recorded in `D/install.json`. Never a shell, never a name looked up on a PATH,
//! always a deadline; one JSON object on stdout is the answer.
//!
//! [`Cli::installed`] is the one place that decides what runs the CLI. Until R4 that is the installed connector
//! (installed by `npx`, or by the app from the same package: `install`); R4 adds the executable the app ships here, and
//! no caller changes. The install's installer runs the same way, through [`Cli::exec`].

use crate::checks::Check;
use crate::paths::DataDirs;
use crate::platform::storage::{Access, Directory, Storage, TrustedStorage};
use crate::Refusal;
use serde_json::Value;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: u64 = 1024 * 1024;
const RECORD_LIMIT: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    /// The native connector executable, followed by any fixed launch arguments.
    /// The current install writer records the executable alone; action arguments are passed to `run`.
    pub prefix: Vec<String>,
    /// `D`, passed down as `SIDEVOICE_DATA_DIR` so the CLI and the app agree on it.
    pub data: std::path::PathBuf,
}

impl Cli {
    /// The CLI the install recorded. `D`'s ancestry safe from other users, `D` itself this user's and not writable by
    /// others, `install.json` read through `D`'s checked descriptor (a regular file, not a link, this user's, not
    /// writable by others), and `command` a non-empty array of absolute paths, each an executable file only root or
    /// this user can change. No install, no CLI (`cli.unavailable`).
    pub fn installed(dirs: &DataDirs) -> Result<Cli, Refusal> {
        let dir = checked(
            Storage::open(&dirs.data, Access::NotWritableByOthers)
                .map_err(|e| crate::checks::storage_error(&dirs.data, "install.unsafe", e)),
        )?;
        let record = dirs.install_record();
        let text = checked(
            dir.read("install.json".as_ref(), Access::NotWritableByOthers, RECORD_LIMIT)
                .map_err(|e| crate::checks::storage_error(&record, "install.unsafe", e))
                .and_then(|bytes| bytes.ok_or(Check::Missing)),
        )?;
        let install: Value = serde_json::from_slice(&text)
            .map_err(|e| Refusal::new("install.unreadable", format!("{} is not JSON: {e}", record.display())))?;
        let prefix = command(&install).ok_or_else(|| {
            Refusal::new("install.unreadable", format!("{} names no command of absolute paths.", record.display()))
        })?;
        // What runs is exactly what was checked: each path resolved once, and `D` as its descriptor was checked.
        let mut verified = Vec::with_capacity(prefix.len());
        for path in &prefix {
            let resolved = checked(
                Storage::trusted_program(std::path::Path::new(path))
                    .map_err(|e| crate::checks::storage_error(std::path::Path::new(path), "install.unsafe", e)),
            )?;
            let resolved = resolved.into_os_string().into_string().map_err(|_| {
                Refusal::new("install.unreadable", format!("{} names a path that is not UTF-8.", record.display()))
            })?;
            verified.push(resolved);
        }
        let data = checked(
            dirs.data.canonicalize().map_err(|e| crate::checks::storage_error(&dirs.data, "install.unsafe", e)),
        )?;
        Ok(Cli { prefix: verified, data })
    }

    /// Runs `prefix + args` and waits at most `timeout` — for the process to exit **and** for both its output streams
    /// to close: its JSON answer when it says `ok`, else its `{key, message}`.
    ///
    /// The run is a process group of its own. Whatever is left in it once the program exits (a child still holding
    /// the pipes) is killed, and on a timeout the whole group is. What the program hands to the service manager
    /// (`service start` → launchd) or starts detached in a session of its own is not in the group, and stays.
    pub fn run(&self, args: &[&str], timeout: Duration) -> Result<Value, Refusal> {
        let failed = |why: String| Refusal::new("cli.failed", why);
        let ran = self.exec(args, timeout, &mut || {})?;
        let Some(status) = ran.status else {
            return Err(Refusal::new(
                "cli.timeout",
                format!("`{}` did not answer within {} s.", args.join(" "), timeout.as_secs()),
            ));
        };
        let stderr = String::from_utf8_lossy(&ran.stderr).into_owned();
        let tail =
            || stderr.lines().rev().take(5).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ");
        let answer: Value = serde_json::from_slice(&ran.stdout)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(|| failed(format!("`{}` exited {status} without a JSON answer: {}", args.join(" "), tail())))?;
        if answer.get("ok") == Some(&Value::Bool(true)) && status.success() {
            return Ok(answer);
        }
        Err(refusal_of(&answer).unwrap_or_else(|| failed(format!("`{}` exited {status}: {}", args.join(" "), tail()))))
    }

    /// Runs `prefix + args` as [`Self::run`] does — a process group of its own, at most `timeout` for its exit and both
    /// its output streams — and hands back what it left, `status` `None` when the deadline came first (the whole group
    /// was killed). `tick` is called about every 50 ms while it runs. Only a program that cannot start is a refusal
    /// (`cli.failed`).
    pub fn exec(&self, args: &[&str], timeout: Duration, tick: &mut dyn FnMut()) -> Result<Ran, Refusal> {
        let failed = |why: String| Refusal::new("cli.failed", why);
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
                spawned => break spawned.map_err(|e| failed(format!("{} could not start: {e}", self.prefix[0])))?,
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
        loop {
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
            if left.is_zero() {
                break;
            }
            drain(&mut streams, left.min(Duration::from_millis(50)));
            tick();
        }
        if status.is_none() {
            kill_group();
            let _ = child.kill();
            let _ = child.wait();
        }
        let [stdout, stderr] = streams.map(|s| s.bytes);
        Ok(Ran { status, stdout, stderr })
    }
}

/// What a run left ([`Cli::exec`]): its exit status, `None` when it was killed at the deadline, and what it wrote (at
/// most 1 MiB of each stream).
#[derive(Debug)]
pub struct Ran {
    pub status: Option<ExitStatus>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// One of the program's output pipes, read without blocking past the deadline.
struct Stream {
    fd: Option<OwnedFd>,
    bytes: Vec<u8>,
}

impl Stream {
    fn new(fd: Option<OwnedFd>) -> Self {
        Stream { fd, bytes: Vec::new() }
    }

    fn is_open(&self) -> bool {
        self.fd.is_some()
    }
}

/// Waits at most `wait` for any open stream to have something, and reads what there is. End of file, or an error,
/// closes the stream; beyond [`OUTPUT_LIMIT`] bytes are read and dropped.
fn drain(streams: &mut [Stream], wait: Duration) {
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
    for stream in streams.iter_mut().filter(|s| s.fd.is_some()) {
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
    }
}

/// A check's outcome for the CLI: missing is "not installed".
fn checked<T>(result: Result<T, Check>) -> Result<T, Refusal> {
    result.map_err(|check| match check {
        Check::Missing => Refusal::new("cli.unavailable", "Sidevoice is not installed on this computer."),
        Check::Unsafe(refusal) => refusal,
    })
}

/// `install.json`'s `command`: every element an absolute path.
fn command(install: &Value) -> Option<Vec<String>> {
    let items = install.get("command")?.as_array()?;
    let prefix: Vec<String> = items.iter().map(|v| v.as_str().map(str::to_string)).collect::<Option<_>>()?;
    let absolute = |p: &String| p.starts_with('/') && !p.contains('\0');
    (!prefix.is_empty() && prefix.iter().all(absolute)).then_some(prefix)
}

/// `{ok: false, error: {key, message}}` (SEAMS §3).
fn refusal_of(answer: &Value) -> Option<Refusal> {
    let error = answer.get("error")?;
    let key = error.get("key")?.as_str()?;
    let message = error.get("message").and_then(Value::as_str).unwrap_or(key);
    Some(Refusal::new(key, message))
}
