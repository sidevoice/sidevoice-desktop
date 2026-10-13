//! What every command shares: the repository, the app's version, running programs (with secrets kept out of what is
//! printed), programs in the background, and reading the logs the app writes with `SIDEVOICE_DEBUG=1`.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use regex::Regex;

use crate::Result;

/// The repository's root: xtask/ is a package of its own, one level down.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask/ is in the repository").to_path_buf()
}

/// Where the commands keep their logs and scratch files: `target/xtask/<name>` in the repository (ignored by git).
pub fn work(name: &str) -> Result<PathBuf> {
    let dir = repo().join("target/xtask").join(name);
    mkdir(&dir)?;
    Ok(dir)
}

/// The app's one version (src-tauri/tauri.conf.json; release-please moves it with the others).
pub fn version() -> Result<String> {
    let path = repo().join("src-tauri/tauri.conf.json");
    let conf: serde_json::Value =
        serde_json::from_slice(&read_bytes(&path)?).map_err(|e| format!("{}: {e}", path.display()))?;
    conf["version"].as_str().map(str::to_owned).ok_or_else(|| format!("{} names no version", path.display()))
}

pub fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn write(path: &Path, bytes: impl AsRef<[u8]>) -> Result<()> {
    fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn mkdir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// An empty directory at `path` (whatever was there is removed).
pub fn fresh_dir(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    mkdir(path)
}

/// Copies the directory `from` to `to` (which must not exist yet), files and subdirectories.
pub fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    mkdir(to)?;
    for entry in fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        if source.is_dir() {
            copy_dir(&source, &target)?;
        } else {
            fs::copy(&source, &target).map_err(|e| format!("{}: {e}", source.display()))?;
        }
    }
    Ok(())
}

/// The files directly in `dir`, sorted by name.
pub fn files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    Ok(files)
}

pub fn name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn sleep(seconds: f64) {
    thread::sleep(Duration::from_secs_f64(seconds));
}

/// Whether this runs on a CI runner (GitHub Actions sets `CI=true`): a machine discarded after the job.
pub fn in_ci() -> bool {
    std::env::var("CI").is_ok_and(|ci| ci == "true")
}

/// The variable `name`, which must be set and not empty. Its value is never printed.
pub fn required_env(name: &str) -> Result<String> {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Ok(value),
        _ => Err(format!("{name} is not set (RELEASING.md, \"macOS signing and notarization\")")),
    }
}

/// A program to run, with what is printed of it: each argument as given, or `***` for a secret one.
pub struct Cmd {
    command: Command,
    shown: Vec<String>,
}

impl Cmd {
    /// `program`, run in the repository's root.
    ///
    /// `cargo xtask` runs under rustup's proxy, which sets `RUSTUP_TOOLCHAIN` to the toolchain that built the xtask:
    /// passed on, it would override `src-tauri/rust-toolchain.toml` for every `cargo` below (the Tauri CLI's
    /// included). So it is dropped unless the person set it themselves (rustup says so in
    /// `RUSTUP_TOOLCHAIN_SOURCE=env`), and so is `CARGO`, the xtask's own cargo.
    pub fn new(program: impl AsRef<OsStr>) -> Cmd {
        let mut command = Command::new(program.as_ref());
        command.current_dir(repo());
        if std::env::var("RUSTUP_TOOLCHAIN_SOURCE").map_or(true, |source| source != "env") {
            command.env_remove("RUSTUP_TOOLCHAIN").env_remove("RUSTUP_TOOLCHAIN_SOURCE");
        }
        command.env_remove("CARGO");
        Cmd { command, shown: vec![program.as_ref().to_string_lossy().into_owned()] }
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Cmd {
        self.shown.push(arg.as_ref().to_string_lossy().into_owned());
        self.command.arg(arg);
        self
    }

    pub fn args<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(mut self, args: I) -> Cmd {
        for arg in args {
            self = self.arg(arg);
        }
        self
    }

    /// An argument that is a secret (a password, a key's id): passed as it is, printed as `***`.
    pub fn secret(mut self, arg: impl AsRef<OsStr>) -> Cmd {
        self.shown.push("***".into());
        self.command.arg(arg);
        self
    }

    pub fn dir(mut self, dir: impl AsRef<Path>) -> Cmd {
        self.command.current_dir(dir);
        self
    }

    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Cmd {
        self.command.env(key, value);
        self
    }

    pub fn shown(&self) -> String {
        self.shown.join(" ")
    }

    /// Runs it, its output going through; fails unless it succeeds.
    pub fn run(mut self) -> Result<()> {
        let what = self.shown();
        eprintln!("$ {what}");
        let status = self.command.status().map_err(|e| format!("{what}: {e}"))?;
        status.success().then_some(()).ok_or_else(|| format!("{what}: {status}"))
    }

    /// Runs it and returns its standard output then its standard error (as `2>&1` had them); fails unless it
    /// succeeds.
    pub fn output(self) -> Result<String> {
        let (ok, text, what) = self.capture(true)?;
        ok.then_some(text.clone()).ok_or_else(|| format!("{what} failed:\n{text}"))
    }

    /// Runs it and returns its standard output alone (its standard error goes through); fails unless it succeeds.
    pub fn stdout(self) -> Result<String> {
        let (ok, text, what) = self.capture(false)?;
        ok.then_some(text).ok_or_else(|| format!("{what} failed"))
    }

    /// Runs it and returns whether it succeeded, with its output (both streams); never fails for its status.
    pub fn attempt(self) -> Result<(bool, String)> {
        let (ok, text, _) = self.capture(true)?;
        Ok((ok, text))
    }

    /// Runs it with `input` on its standard input and its standard output into `out`; fails unless it succeeds.
    pub fn pipe(mut self, input: &[u8], out: File) -> Result<()> {
        let what = self.shown();
        eprintln!("$ {what} < (input) > (file)");
        let mut child =
            self.command.stdin(Stdio::piped()).stdout(Stdio::from(out)).spawn().map_err(|e| format!("{what}: {e}"))?;
        let written = child.stdin.take().expect("piped").write_all(input);
        let status = child.wait().map_err(|e| format!("{what}: {e}"))?;
        written.map_err(|e| format!("{what}: {e}"))?;
        status.success().then_some(()).ok_or_else(|| format!("{what}: {status}"))
    }

    fn capture(mut self, with_stderr: bool) -> Result<(bool, String, String)> {
        let what = self.shown();
        eprintln!("$ {what}");
        if !with_stderr {
            self.command.stderr(Stdio::inherit());
        }
        let out = self.command.output().map_err(|e| format!("{what}: {e}"))?;
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        if with_stderr {
            text += &String::from_utf8_lossy(&out.stderr);
        }
        Ok((out.status.success(), text, what))
    }

    /// Starts it in the background, its standard output and error both into `log` (created empty).
    pub fn spawn(mut self, log: &Path) -> Result<Background> {
        let what = self.shown();
        let file = File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
        let err = file.try_clone().map_err(|e| e.to_string())?;
        eprintln!("$ {what} > {} 2>&1 &", log.display());
        let child = self
            .command
            .stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::from(err))
            .spawn()
            .map_err(|e| format!("{what}: {e}"))?;
        Ok(Background { child, readers: Vec::new() })
    }

    /// Starts it in the background, each line it writes (either stream) into `log` with the time before it
    /// (`<seconds since 1970, to the hundredth> <line>`), to order it against other logs.
    pub fn spawn_timed(mut self, log: &Path) -> Result<Background> {
        let what = self.shown();
        let file = Arc::new(Mutex::new(File::create(log).map_err(|e| format!("{}: {e}", log.display()))?));
        eprintln!("$ {what} > (timed) {} 2>&1 &", log.display());
        let mut child = self
            .command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{what}: {e}"))?;
        let stdout: Box<dyn Read + Send> = Box::new(child.stdout.take().expect("piped"));
        let stderr: Box<dyn Read + Send> = Box::new(child.stderr.take().expect("piped"));
        let readers = [stdout, stderr]
            .into_iter()
            .map(|stream| {
                let file = Arc::clone(&file);
                thread::spawn(move || {
                    let mut reader = BufReader::new(stream);
                    let mut line = Vec::new();
                    while reader.read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
                        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64();
                        let mut file = file.lock().expect("the log's lock");
                        let _ = write!(file, "{now:.2} ");
                        let _ = file.write_all(&line);
                        let _ = file.flush();
                        line.clear();
                    }
                })
            })
            .collect();
        Ok(Background { child, readers })
    }
}

/// A program running in the background. Stopped (as `kill` stops it: SIGTERM) when dropped.
pub struct Background {
    child: Child,
    readers: Vec<thread::JoinHandle<()>>,
}

impl Background {
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Asks it to stop (SIGTERM, as `kill PID` did), and waits a few seconds for it before killing it outright.
    pub fn stop(&mut self) {
        if !self.running() {
            return self.join();
        }
        terminate(self.child.id());
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.running() && Instant::now() < deadline {
            sleep(0.1);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.join();
    }

    /// Waits (a few seconds at most: a helper process may hold the pipes open) for the last lines to reach the log.
    fn join(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.readers.iter().any(|reader| !reader.is_finished()) && Instant::now() < deadline {
            sleep(0.05);
        }
        self.readers.retain(|reader| !reader.is_finished());
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        self.stop();
    }
}

/// SIGTERM to `pid` (`kill PID`); nothing if it is gone.
pub fn terminate(pid: u32) {
    #[cfg(unix)]
    let _ = Command::new("kill").arg(pid.to_string()).stderr(Stdio::null()).status();
    #[cfg(not(unix))]
    let _ = Command::new("taskkill").args(["/PID", &pid.to_string()]).stderr(Stdio::null()).status();
}

/// A log's text so far ("" when there is none yet).
pub fn read(log: &Path) -> String {
    fs::read(log).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default()
}

/// A regular expression written in the code: it compiles.
pub fn re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|e| panic!("the pattern {pattern:?}: {e}"))
}

/// The lines of `text` after its first `after` (as `tail -n +<after + 1>` gives them), the last one even if it is
/// not finished yet.
pub fn lines_after(text: &str, after: usize) -> impl Iterator<Item = &str> {
    text.split_inclusive('\n').skip(after).map(|line| line.trim_end_matches('\n'))
}

/// Whether a line of `text` matches `pattern` (`grep -q`).
pub fn has(text: &str, pattern: &str) -> bool {
    let pattern = re(pattern);
    text.lines().any(|line| pattern.is_match(line))
}

/// The lines of `text` that match `pattern` (`grep`).
pub fn grep<'a>(text: &'a str, pattern: &str) -> Vec<&'a str> {
    let pattern = re(pattern);
    text.lines().filter(|line| pattern.is_match(line)).collect()
}

/// How many lines `log` has now (`wc -l`: its finished lines).
pub fn line_count(log: &Path) -> usize {
    read(log).bytes().filter(|&b| b == b'\n').count()
}

/// Waits at most `seconds` (checking every second, as the YAML's loops did) for a line of `log`, after its first
/// `after`, that matches `pattern`.
pub fn wait_for(log: &Path, pattern: &str, after: usize, seconds: u64) -> bool {
    let pattern = re(pattern);
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        if lines_after(&read(log), after).any(|line| pattern.is_match(line)) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(1.0);
    }
}

/// `Err(what)` unless `ok`: one check of a step that stops at the first that does not hold (as `set -e` did).
pub fn ensure(ok: bool, what: impl Into<String>) -> Result<()> {
    ok.then_some(()).ok_or_else(|| what.into())
}

/// Whether `bytes` holds `text` (`grep -q` on a binary).
pub fn contains(bytes: &[u8], text: &str) -> bool {
    bytes.windows(text.len()).any(|window| window == text.as_bytes())
}

/// The npm launcher: a `.cmd` on Windows.
pub fn npm() -> &'static str {
    if cfg!(windows) {
        "npm.cmd"
    } else {
        "npm"
    }
}

/// A `tauri build` (the Tauri CLI package.json pins).
#[derive(Debug, Default, PartialEq)]
pub struct TauriBuild<'a> {
    /// `--target`: none for the host's own.
    pub target: Option<&'a str>,
    /// `--bundles`: the installers to make.
    pub bundles: &'a str,
    /// `--features` of the app's crate.
    pub features: Option<&'a str>,
    /// `--config`: JSON merged into tauri.conf.json.
    pub config: Option<&'a str>,
}

impl TauriBuild<'_> {
    pub fn args(&self) -> Vec<String> {
        let mut args = vec!["build".to_owned()];
        if let Some(target) = self.target {
            args.extend(["--target".into(), target.into()]);
        }
        args.extend(["--bundles".into(), self.bundles.into()]);
        if let Some(features) = self.features {
            args.extend(["--features".into(), features.into()]);
        }
        if let Some(config) = self.config {
            args.extend(["--config".into(), config.into()]);
        }
        args
    }

    /// The command: the CLI's script run by Node from `node_modules` (what `npx tauri` runs), the same on every OS.
    pub fn command(&self) -> Cmd {
        Cmd::new("node").arg(repo().join("node_modules/@tauri-apps/cli/tauri.js")).args(self.args())
    }
}

/// Where the secrets directory `name` goes: under `RUNNER_TEMP` on a runner, the system's temporary directory
/// elsewhere. A fixed place, so that `macos forget-secrets` finds what a run that was cut short left.
pub fn secret_path(name: &str) -> PathBuf {
    std::env::var_os("RUNNER_TEMP").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join(name)
}

/// A directory for secrets ([`secret_path`]), only this user may enter it (0700), removed with everything in it
/// when dropped.
pub struct SecretDir {
    pub path: PathBuf,
}

impl SecretDir {
    pub fn new(name: &str) -> Result<SecretDir> {
        let path = secret_path(name);
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        private_dir(&path)?;
        Ok(SecretDir { path })
    }

    /// A new file in it holding `bytes`, created readable and writable by this user only (0600).
    pub fn file(&self, name: &str, bytes: &[u8]) -> Result<PathBuf> {
        let path = self.path.join(name);
        let mut file = private_file(&path)?;
        file.write_all(bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }
}

impl Drop for SecretDir {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.path) {
            if self.path.exists() {
                eprintln!("::warning::could not remove {}: {error}", self.path.display());
            }
        }
    }
}

#[cfg(unix)]
fn private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// A new file at `path`, only this user may read or write it (0600); it must not exist yet.
#[cfg(unix)]
pub fn private_file(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(not(unix))]
fn private_dir(_: &Path) -> Result<()> {
    Err("secrets are only written on macOS".into())
}

#[cfg(not(unix))]
pub fn private_file(_: &Path) -> Result<File> {
    Err("secrets are only written on macOS".into())
}

/// The permission bits of `path` (`stat -f %Lp`), on Unix.
#[cfg(unix)]
pub fn mode(path: &Path) -> Result<u32> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(meta.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
pub fn mode(path: &Path) -> Result<u32> {
    Err(format!("{}: no Unix permissions here", path.display()))
}

#[cfg(unix)]
pub fn chmod(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(not(unix))]
pub fn chmod(path: &Path, _: u32) -> Result<()> {
    Err(format!("{}: no Unix permissions here", path.display()))
}

/// `bytes` random bytes from the system, as hex (`openssl rand -hex <bytes>`).
pub fn random_hex(bytes: usize) -> Result<String> {
    let mut buffer = vec![0u8; bytes];
    File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buffer)).map_err(|e| format!("/dev/urandom: {e}"))?;
    Ok(buffer.iter().map(|b| format!("{b:02x}")).collect())
}

/// Requires `os` (`std::env::consts::OS`): the command's tools are that system's.
pub fn require_os(os: &str, command: &str) -> Result<()> {
    ensure(std::env::consts::OS == os, format!("`cargo xtask {command}` runs on {os}"))
}

/// Requires a CI runner: the command changes the machine (adds a user) in a way meant for a throwaway one.
pub fn require_ci(command: &str) -> Result<()> {
    ensure(
        in_ci(),
        format!("`cargo xtask {command}` is for a CI runner (it changes this machine); set CI=true to run it anyway"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tauri_build_is_its_arguments_in_order() {
        let build = TauriBuild { target: Some("aarch64-apple-darwin"), bundles: "app", ..TauriBuild::default() };
        assert_eq!(build.args(), ["build", "--target", "aarch64-apple-darwin", "--bundles", "app"]);
        let probe = TauriBuild { features: Some("probe"), ..build };
        assert_eq!(probe.args().join(" "), "build --target aarch64-apple-darwin --bundles app --features probe");
        let host = TauriBuild { bundles: "deb,appimage", config: Some("{}"), ..TauriBuild::default() };
        assert_eq!(host.args().join(" "), "build --bundles deb,appimage --config {}");
    }

    #[test]
    fn a_secret_argument_is_never_shown() {
        let cmd = Cmd::new("security").args(["unlock-keychain", "-p"]).secret("hunter2").arg("k.keychain-db");
        assert_eq!(cmd.shown(), "security unlock-keychain -p *** k.keychain-db");
    }

    #[test]
    fn lines_after_skips_whole_lines_and_keeps_an_unfinished_one() {
        let text = "a\nb\nc";
        assert_eq!(lines_after(text, 0).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(lines_after(text, 2).collect::<Vec<_>>(), ["c"]);
        assert_eq!(lines_after(text, 3).count(), 0);
    }

    #[test]
    fn has_and_grep_go_line_by_line() {
        let log = "x local-host-flow ok version=2\ny local-host-flow error\n";
        assert!(has(log, "local-host-flow (ok|error)"));
        assert!(!has(log, "ok.*error"));
        assert_eq!(grep(log, "flow ok"), ["x local-host-flow ok version=2"]);
    }

    #[test]
    fn a_binary_contains_a_marker() {
        assert!(contains(b"\0\0SIDEVOICE_DEBUG_PAGE\0", "SIDEVOICE_DEBUG_PAGE"));
        assert!(!contains(b"SIDEVOICE_DEBUG_PAG", "SIDEVOICE_DEBUG_PAGE"));
    }
}
