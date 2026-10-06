//! What every command shares: the repository, the app's version, running programs, and reading the logs the app
//! writes with `SIDEVOICE_DEBUG=1`.

use crate::Result;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// The repository's root (the workspace this crate is in).
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask is one level down").to_path_buf()
}

/// The workspace's build directory.
pub fn target() -> PathBuf {
    repo().join("target")
}

/// The app's one version (src-tauri/tauri.conf.json; release-please moves it with the others).
pub fn version() -> Result<String> {
    let path = repo().join("src-tauri/tauri.conf.json");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let conf: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    conf["version"].as_str().map(str::to_owned).ok_or_else(|| format!("{} names no version", path.display()))
}

/// The pinned Tauri CLI (package.json), run by Node from `node_modules`: the same on every OS, no `npx` shim.
pub fn tauri(args: &[&str]) -> Command {
    let mut command = Command::new("node");
    command.arg(repo().join("node_modules/@tauri-apps/cli/tauri.js")).args(args).current_dir(repo());
    command
}

fn describe(command: &Command) -> String {
    let mut words = vec![command.get_program().to_string_lossy().into_owned()];
    words.extend(command.get_args().map(|a| a.to_string_lossy().into_owned()));
    words.join(" ")
}

/// Runs `command` with the output going through; fails unless it succeeds.
pub fn run(command: &mut Command) -> Result<()> {
    let what = describe(command);
    eprintln!("$ {what}");
    let status = command.status().map_err(|e| format!("{what}: {e}"))?;
    status.success().then_some(()).ok_or_else(|| format!("{what}: {status}"))
}

/// Runs `command` and returns its standard output (and error, after it); fails unless it succeeds.
pub fn output(command: &mut Command) -> Result<String> {
    let what = describe(command);
    let out = command.output().map_err(|e| format!("{what}: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    out.status.success().then_some(text.clone()).ok_or_else(|| format!("{what}: {}\n{text}", out.status))
}

/// Requires `CI`: the command changes the machine (users, the app's data) in ways meant for a throwaway runner.
pub fn require_ci(what: &str) -> Result<()> {
    match std::env::var("CI").as_deref() {
        Ok("true") => Ok(()),
        _ => Err(format!("`{what}` is meant for a CI runner (it changes this machine); set CI=true to run it anyway")),
    }
}

/// An empty directory at `path` (whatever was there is removed).
pub fn fresh_dir(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    fs::create_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn sleep(seconds: f64) {
    thread::sleep(Duration::from_secs_f64(seconds));
}

/// A program running in the background, its standard output and error both into one log; killed when dropped.
pub struct Background {
    pub child: Child,
}

impl Background {
    pub fn start(command: &mut Command, log: &Path) -> Result<Background> {
        let what = describe(command);
        let file = File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
        let err = file.try_clone().map_err(|e| e.to_string())?;
        eprintln!("$ {what} > {}", log.display());
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::from(err))
            .spawn()
            .map_err(|e| format!("{what}: {e}"))?;
        Ok(Background { child })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A log's text so far ("" when there is none yet).
pub fn read(log: &Path) -> String {
    fs::read(log).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default()
}

/// Whether `line` holds the parts of `pattern` separated by `.*`, in that order.
pub fn matches(line: &str, pattern: &str) -> bool {
    let mut rest = line;
    for part in pattern.split(".*") {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

/// The lines of `text` after the first `after`, that [`matches`] `pattern`.
pub fn lines_matching<'a>(text: &'a str, pattern: &'a str, after: usize) -> impl Iterator<Item = &'a str> + 'a {
    text.lines().skip(after).filter(move |line| matches(line, pattern))
}

/// Whether any line of `text` [`matches`] `pattern`.
pub fn has(text: &str, pattern: &str) -> bool {
    lines_matching(text, pattern, 0).next().is_some()
}

/// Waits at most `seconds` for a line of `log`, after its first `after` lines, that [`matches`] `pattern`.
pub fn wait_for(log: &Path, pattern: &str, after: usize, seconds: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        if lines_matching(&read(log), pattern, after).next().is_some() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(0.5);
    }
}

/// How many lines `log` has now.
pub fn line_count(log: &Path) -> usize {
    read(log).lines().count()
}

/// The number right after `key` in `text`, and what follows it.
pub fn number_after<'a>(text: &'a str, key: &str) -> Option<(u64, &'a str)> {
    let at = text.find(key)? + key.len();
    let rest = &text[at..];
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let number = rest[..digits].parse().ok()?;
    Some((number, &rest[digits..]))
}

/// The word right after `key` in `text`, up to the next space.
pub fn word_after<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let at = text.find(key)? + key.len();
    Some(text[at..].split(' ').next().unwrap_or(""))
}

/// Collects failed checks so a flow runs every check and fails at the end with all of them.
#[derive(Default)]
pub struct Checks {
    failed: Vec<String>,
}

impl Checks {
    pub fn check(&mut self, ok: bool, what: impl Into<String>) {
        if !ok {
            let what = what.into();
            eprintln!("::error::{what}");
            self.failed.push(what);
        }
    }

    pub fn fail(&mut self, what: impl Into<String>) {
        self.check(false, what);
    }

    pub fn done(self, flow: &str) -> Result<()> {
        if self.failed.is_empty() {
            eprintln!("{flow}: every check held");
            Ok(())
        } else {
            Err(format!("{flow}: {} check(s) did not hold:\n  {}", self.failed.len(), self.failed.join("\n  ")))
        }
    }
}
