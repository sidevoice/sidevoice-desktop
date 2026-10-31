//! The connector's CLI, as the app runs it (SEAMS §3): `command + [subcommand…, --json]`, where `command` is the
//! absolute argv prefix the install recorded in `D/install.json`. Never a shell, never a name looked up on a PATH,
//! always a deadline; one JSON object on stdout is the answer.
//!
//! [`Cli::installed`] is the one place that decides what runs the CLI. Until R4 that is the installed connector
//! (installed by `npx`, or by the app from the same package: `install`); R4 adds the executable the app ships here, and
//! no caller changes. The install's installer runs the same way, through [`Cli::exec`].

use crate::checks::Check;
use crate::paths::DataDirs;
use crate::Refusal;
use serde_json::Value;
use sidevoice_platform::fs::{Access, Directory, Files, Fs, HeldDirectory};
use sidevoice_platform::process::{Process, Processes};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

const OUTPUT_LIMIT: usize = 1024 * 1024;
/// How long the output may stay open once the program has exited (a child of its own still holding the pipes) before
/// the rest of its tree is killed.
const PIPE_GRACE: Duration = Duration::from_millis(500);
/// How often [`Cli::exec`] calls its `tick` while the program runs.
const TICK: Duration = Duration::from_millis(50);
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
            HeldDirectory::open(&dirs.data, Access::NotWritableByOthers)
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
                Files::trusted_program(std::path::Path::new(path))
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

    /// Runs `prefix + args` and waits at most `timeout` for the process to exit, then at most [`PIPE_GRACE`] for both
    /// its output streams to close: its JSON answer when it says `ok`, else its `{key, message}`.
    ///
    /// The run is a tree of its own (`sidevoice_platform::process`: a process group, a Job Object on Windows). Whatever
    /// is left in it once the program has exited (a child still holding the pipes) is killed at most [`PIPE_GRACE`]
    /// later, and on a timeout the whole tree is. What the program hands to the service manager
    /// (`service start` → launchd) is not in the tree, and stays; so does what it starts detached where that leaves the
    /// tree (a session of its own), and where the tree holds it (a Job Object allows no breakaway) it is killed with the
    /// run.
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

    /// Runs `prefix + args` as [`Self::run`] does — in the program's own directory, a tree of its own, at most `timeout`
    /// for the program's exit, then at most [`PIPE_GRACE`] for both its output streams to close — and hands back what it
    /// left, `status` `None` when the deadline came first (the whole tree was killed; what it wrote is not kept then).
    /// `tick` is called about every 50 ms while it runs. Only a program that cannot start, or whose output cannot be read, is a refusal
    /// (`cli.failed`).
    pub fn exec(&self, args: &[&str], timeout: Duration, tick: &mut dyn FnMut()) -> Result<Ran, Refusal> {
        let failed = |why: String| Refusal::new("cli.failed", why);
        let mut command = tokio::process::Command::new(&self.prefix[0]);
        // The program's own directory, checked with it, as the working directory: never the app's, which can be anywhere
        // (on Windows the current directory is also where a DLL loaded by bare name may be found).
        if let Some(dir) = std::path::Path::new(&self.prefix[0]).parent() {
            command.current_dir(dir);
        }
        command
            .args(&self.prefix[1..])
            .args(args)
            .env("SIDEVOICE_DATA_DIR", &self.data)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = crate::runtime::block_on(async {
            let child = Processes::spawn_tree(command)
                .await
                .map_err(|e| failed(format!("{} could not start: {e}", self.prefix[0])))?;
            let output = child.output(timeout, PIPE_GRACE, OUTPUT_LIMIT);
            tokio::pin!(output);
            let mut every = tokio::time::interval(TICK);
            loop {
                tokio::select! {
                    output = &mut output => return Ok::<_, Refusal>(output),
                    _ = every.tick() => tick(),
                }
            }
        })?;
        match output {
            Ok(output) => Ok(Ran { status: Some(output.status), stdout: output.stdout, stderr: output.stderr }),
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                Ok(Ran { status: None, stdout: Vec::new(), stderr: Vec::new() })
            }
            Err(e) => Err(failed(format!("{}'s output could not be read: {e}", self.prefix[0]))),
        }
    }
}

/// What a run left ([`Cli::exec`]): its exit status, `None` when it was killed at the deadline, and what it wrote (at
/// most 1 MiB of each stream; nothing when it was killed).
#[derive(Debug)]
pub struct Ran {
    pub status: Option<ExitStatus>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
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
    let absolute = |p: &String| std::path::Path::new(p).is_absolute() && !p.contains('\0');
    (!prefix.is_empty() && prefix.iter().all(absolute)).then_some(prefix)
}

/// `{ok: false, error: {key, message}}` (SEAMS §3).
fn refusal_of(answer: &Value) -> Option<Refusal> {
    let error = answer.get("error")?;
    let key = error.get("key")?.as_str()?;
    let message = error.get("message").and_then(Value::as_str).unwrap_or(key);
    Some(Refusal::new(key, message))
}
