//! The connector's CLI, as the app runs it (SEAMS §3): `command + [subcommand…, --json]`, where `command` is the
//! absolute argv prefix the install recorded in `D/install.json`. Never a shell, never a name looked up on a PATH,
//! always a deadline; one JSON object on stdout is the answer.
//!
//! [`Cli::installed`] is the one place that decides what runs the CLI. Until R4 that is the `npx`-installed
//! connector; R4 adds the executable the app ships here, and no caller changes.

use crate::checks::Check;
use crate::paths::DataDirs;
use crate::trusted;
use crate::Refusal;
use serde_json::Value;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
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

impl Cli {
    /// The CLI the install recorded. `D`'s ancestry safe from other users, `D` itself this user's and not writable by
    /// others, `install.json` read through `D`'s checked descriptor (a regular file, not a link, this user's, not
    /// writable by others), and `command` a non-empty array of absolute paths, each a program or file only root or
    /// this user can change. No install, no CLI (`cli.unavailable`).
    pub fn installed(dirs: &DataDirs) -> Result<Cli, Refusal> {
        let dir = checked(trusted::Dir::open(&dirs.data, 0o022, "install.unsafe"))?;
        let text = checked(dir.read("install.json", 0o022, RECORD_LIMIT))?;
        let record = dirs.install_record();
        let install: Value = serde_json::from_slice(&text)
            .map_err(|e| Refusal::new("install.unreadable", format!("{} is not JSON: {e}", record.display())))?;
        let prefix = command(&install).ok_or_else(|| {
            Refusal::new("install.unreadable", format!("{} names no command of absolute paths.", record.display()))
        })?;
        for path in &prefix {
            checked(trusted::executable(std::path::Path::new(path)))?;
        }
        Ok(Cli { prefix, data: dirs.data.clone() })
    }

    /// Runs `prefix + args` and waits at most `timeout` — for the process to exit **and** for both its output streams
    /// to close: its JSON answer when it says `ok`, else its `{key, message}`.
    ///
    /// The run is a process group of its own. Whatever is left in it once the program exits (a child still holding
    /// the pipes) is killed, and on a timeout the whole group is. What the program hands to the service manager
    /// (`service start` → launchd) or starts detached in a session of its own is not in the group, and stays.
    pub fn run(&self, args: &[&str], timeout: Duration) -> Result<Value, Refusal> {
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
        let [stdout, stderr] = streams.map(|s| s.bytes);
        let stderr = String::from_utf8_lossy(&stderr).into_owned();
        let tail =
            || stderr.lines().rev().take(5).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ");
        let answer: Value = serde_json::from_slice(&stdout)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(|| failed(format!("`{}` exited {status} without a JSON answer: {}", args.join(" "), tail())))?;
        if answer.get("ok") == Some(&Value::Bool(true)) && status.success() {
            return Ok(answer);
        }
        Err(refusal_of(&answer).unwrap_or_else(|| failed(format!("`{}` exited {status}: {}", args.join(" "), tail()))))
    }
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
        let cli = fake(tmp.path(), r#"echo '{"ok":false,"error":{"key":"service.not-loaded","message":"m"}}'; exit 1"#);
        let refusal = cli.run(&["service", "start", "--json"], Duration::from_secs(5)).unwrap_err();
        assert_eq!(refusal, Refusal::new("service.not-loaded", "m"));
    }

    #[test]
    fn no_json_a_crash_or_a_hang_are_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = fake(tmp.path(), "echo boom >&2; exit 3");
        let refusal = cli.run(&["x"], Duration::from_secs(5)).unwrap_err();
        assert_eq!(refusal.key, "cli.failed");
        assert!(refusal.message.contains("boom"), "{}", refusal.message);
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
        assert_eq!(cli.prefix, ["/bin/sh".to_string(), script.display().to_string()]);
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
