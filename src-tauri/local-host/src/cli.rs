//! The connector's CLI, as the app runs it (SEAMS §3): `command + [subcommand…, --json]`, where `command` is the
//! absolute argv prefix the install recorded in `D/install.json`. Never a shell, never a name looked up on a PATH,
//! always a deadline; one JSON object on stdout is the answer.
//!
//! [`Cli::installed`] is the one place that decides what runs the CLI. Until R4 that is the `npx`-installed
//! connector; R4 adds the executable the app ships here, and no caller changes.

use crate::checks::{self, Check};
use crate::paths::DataDirs;
use crate::Refusal;
use serde_json::Value;
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    /// Absolute paths: the program, then what it needs before the subcommand (`[node, …/cli.mjs]`).
    pub prefix: Vec<String>,
    /// `D`, passed down as `SIDEVOICE_DATA_DIR` so the CLI and the app agree on it.
    pub data: std::path::PathBuf,
}

impl Cli {
    /// The CLI the install recorded: `D` and `install.json` this user's and not writable by others, and `command` a
    /// non-empty array of absolute paths. No install, no CLI (`cli.unavailable`).
    pub fn installed(dirs: &DataDirs) -> Result<Cli, Refusal> {
        let unavailable = || Refusal::new("cli.unavailable", "Sidevoice is not installed on this computer.");
        let record = dirs.install_record();
        for path in [&dirs.data, &record] {
            match checks::owned_not_writable(path) {
                Ok(()) => {}
                Err(Check::Missing) => return Err(unavailable()),
                Err(Check::Unsafe(refusal)) => return Err(refusal),
            }
        }
        let text = std::fs::read(&record).map_err(|_| unavailable())?;
        let install: Value = serde_json::from_slice(&text)
            .map_err(|e| Refusal::new("install.unreadable", format!("{} is not JSON: {e}", record.display())))?;
        let prefix = command(&install).ok_or_else(|| {
            Refusal::new("install.unreadable", format!("{} names no command of absolute paths.", record.display()))
        })?;
        Ok(Cli { prefix, data: dirs.data.clone() })
    }

    /// Runs `prefix + args` and waits at most `timeout`: its JSON answer when it says `ok`, else its `{key, message}`.
    pub fn run(&self, args: &[&str], timeout: Duration) -> Result<Value, Refusal> {
        let failed = |why: String| Refusal::new("cli.failed", why);
        let mut command = Command::new(&self.prefix[0]);
        command
            .args(&self.prefix[1..])
            .args(args)
            .env("SIDEVOICE_DATA_DIR", &self.data)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
        let drain = |pipe: Option<Box<dyn Read + Send>>| {
            thread::spawn(move || {
                let mut out = Vec::new();
                if let Some(pipe) = pipe {
                    let _ = pipe.take(OUTPUT_LIMIT).read_to_end(&mut out);
                }
                out
            })
        };
        let stdout = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let stderr = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
                Ok(None) | Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Refusal::new(
                        "cli.timeout",
                        format!("`{}` did not answer within {} s.", args.join(" "), timeout.as_secs()),
                    ));
                }
            }
        };
        let stdout = stdout.join().unwrap_or_default();
        let stderr = String::from_utf8_lossy(&stderr.join().unwrap_or_default()).into_owned();
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
        write(r#"{"command":["/usr/bin/node","/home/u/.sidevoice/copies/1/dist/cli.mjs"]}"#, 0o600);
        let cli = Cli::installed(&dirs).unwrap();
        assert_eq!(cli.prefix, ["/usr/bin/node", "/home/u/.sidevoice/copies/1/dist/cli.mjs"]);
        for bad in [r#"{"command":["node","cli.mjs"]}"#, r#"{"command":[]}"#, r#"{"command":"/bin/sh"}"#, "{}", "nope"]
        {
            write(bad, 0o600);
            let key = Cli::installed(&dirs).unwrap_err().key;
            assert_eq!(key, "install.unreadable", "{bad}");
        }
        write(r#"{"command":["/usr/bin/node"]}"#, 0o666);
        assert_eq!(Cli::installed(&dirs).unwrap_err().key, "install.unsafe");
        write(r#"{"command":["/usr/bin/node"]}"#, 0o600);
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(Cli::installed(&dirs).unwrap_err().key, "install.unsafe");
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}
