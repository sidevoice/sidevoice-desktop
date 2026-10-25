// Existing OS calls are reported by the separate Platform boundary CI job.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

//! What every command shares: the repository, the app's version, running programs (with secrets kept out of what is
//! printed), programs in the background, and reading the logs the app writes with `SIDEVOICE_DEBUG=1`.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use regex::Regex;
use sidevoice_platform::fs::{Files, Fs};
use sidevoice_platform::process::{Child, Process, Processes};
use sidevoice_platform::stop::{StopRequest, StopRequests};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

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

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Ctrl+C on every host, and SIGTERM/SIGHUP on Unix, stop new work and unwind resource guards.
/// Children have their own groups/jobs, so running commands also observe this flag and stop their owned trees.
pub fn on_interrupt() -> Result<()> {
    ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::SeqCst)).map_err(|e| e.to_string())
}

/// Keep async child IO alive between synchronous xtask steps (including background log readers).
fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("xtask child runtime"))
}

async fn cancelled() {
    while interrupted().is_ok() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Ask only an owned, still-waitable leader; always clean up its tree, even after the leader exits.
async fn stop_child(child: &mut Child) {
    if matches!(child.try_wait(), Ok(None)) {
        if let Some(pid) = child.id() {
            let _ = StopRequests::ask(pid);
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    }
    child.kill_tree().await;
}

/// `Err` once the xtask was interrupted: nothing more is started.
pub fn interrupted() -> Result<()> {
    ensure(!INTERRUPTED.load(Ordering::SeqCst), "interrupted")
}

/// Whether this runs on a CI runner (GitHub Actions sets `CI=true`): a machine discarded after the job.
pub fn in_ci() -> bool {
    std::env::var("CI").is_ok_and(|ci| ci == "true")
}

/// The variable `name`, which must be set and not empty. Its value is never printed.
pub fn required_env(name: &str) -> Result<String> {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Ok(value),
        _ => Err(format!("{name} is not set (RELEASING.md: the signing secrets)")),
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

    pub fn env_remove(mut self, key: impl AsRef<OsStr>) -> Cmd {
        self.command.env_remove(key);
        self
    }

    pub fn shown(&self) -> String {
        self.shown.join(" ")
    }

    /// Runs it, its output going through; fails unless it succeeds.
    pub fn run(self) -> Result<()> {
        interrupted()?;
        let what = self.shown();
        eprintln!("$ {what}");
        runtime().block_on(async {
            let mut child = Processes::spawn_tree(self.command).await.map_err(|e| format!("{what}: {e}"))?;
            let status = tokio::select! {
                status = child.wait() => status.map_err(|e| format!("{what}: {e}")),
                () = cancelled() => Err("interrupted".into()),
            };
            if interrupted().is_err() {
                stop_child(&mut child).await;
            } else {
                child.kill_tree().await;
            }
            let status = status?;
            status.success().then_some(()).ok_or_else(|| format!("{what}: {status}"))
        })
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
        interrupted()?;
        let what = self.shown();
        eprintln!("$ {what} < (input) > (file)");
        self.command.stdin(Stdio::piped()).stdout(Stdio::from(out));
        runtime().block_on(async {
            let mut child = Processes::spawn_tree(self.command).await.map_err(|e| format!("{what}: {e}"))?;
            let mut stdin = child.stdin().expect("piped");
            let result = tokio::select! {
                result = async {
                    stdin.write_all(input).await.map_err(|e| format!("{what}: {e}"))?;
                    drop(stdin);
                    let status = child.wait().await.map_err(|e| format!("{what}: {e}"))?;
                    status.success().then_some(()).ok_or_else(|| format!("{what}: {status}"))
                } => result,
                () = cancelled() => Err("interrupted".into()),
            };
            if interrupted().is_err() {
                stop_child(&mut child).await;
            } else {
                child.kill_tree().await;
            }
            result
        })
    }

    fn capture(mut self, with_stderr: bool) -> Result<(bool, String, String)> {
        interrupted()?;
        let what = self.shown();
        eprintln!("$ {what}");
        self.command.stdin(Stdio::null()).stdout(Stdio::piped());
        self.command.stderr(if with_stderr { Stdio::piped() } else { Stdio::inherit() });
        let out = runtime().block_on(async {
            let child = Processes::spawn_tree(self.command).await.map_err(|e| format!("{what}: {e}"))?;
            // No new command deadline or output cap: build/release commands can be long and verbose.
            tokio::select! {
                out = child.output(Duration::from_secs(100 * 365 * 24 * 3600), Duration::from_secs(3), usize::MAX) =>
                    out.map_err(|e| format!("{what}: {e}")),
                () = cancelled() => Err("interrupted".into()),
            }
        })?;
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        if with_stderr {
            text += &String::from_utf8_lossy(&out.stderr);
        }
        Ok((out.status.success(), text, what))
    }

    /// Starts it in the background, its standard output and error both into `log` (created empty).
    pub fn spawn(mut self, log: &Path) -> Result<Background> {
        interrupted()?;
        let what = self.shown();
        let file = File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
        let err = file.try_clone().map_err(|e| e.to_string())?;
        eprintln!("$ {what} > {} 2>&1 &", log.display());
        self.command.stdin(Stdio::null()).stdout(Stdio::from(file)).stderr(Stdio::from(err));
        let child = runtime().block_on(Processes::spawn_tree(self.command)).map_err(|e| format!("{what}: {e}"))?;
        Ok(Background { pid: child.id().expect("owned child leader"), child, readers: Vec::new() })
    }

    /// Starts it in the background, each line it writes (either stream) into `log` with the time before it
    /// (`<seconds since 1970, to the hundredth> <line>`), to order it against other logs.
    pub fn spawn_timed(mut self, log: &Path) -> Result<Background> {
        interrupted()?;
        let what = self.shown();
        let file = Arc::new(Mutex::new(File::create(log).map_err(|e| format!("{}: {e}", log.display()))?));
        eprintln!("$ {what} > (timed) {} 2>&1 &", log.display());
        self.command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = runtime().block_on(Processes::spawn_tree(self.command)).map_err(|e| format!("{what}: {e}"))?;
        let stdout: Box<dyn AsyncRead + Send + Unpin> = Box::new(child.stdout().expect("piped"));
        let stderr: Box<dyn AsyncRead + Send + Unpin> = Box::new(child.stderr().expect("piped"));
        let readers = [stdout, stderr]
            .into_iter()
            .map(|stream| {
                let file = Arc::clone(&file);
                runtime().spawn(async move {
                    let mut reader = BufReader::new(stream);
                    let mut line = Vec::new();
                    while reader.read_until(b'\n', &mut line).await.is_ok_and(|n| n > 0) {
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
        Ok(Background { pid: child.id().expect("owned child leader"), child, readers })
    }
}

/// An owned background tree, asked to leave and then killed whole when dropped.
pub struct Background {
    child: Child,
    pid: u32,
    readers: Vec<JoinHandle<()>>,
}

impl Background {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Waits at most `seconds` for it to end on its own: how it ended, or `None` while it still runs.
    pub fn ended(&mut self, seconds: u64) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                self.join();
                return Some(status);
            }
            if Instant::now() >= deadline || interrupted().is_err() {
                return None;
            }
            sleep(0.2);
        }
    }

    /// Asks the owned leader to stop, gives it five seconds, then kills/reaps the whole group or job.
    pub fn stop(&mut self) {
        runtime().block_on(stop_child(&mut self.child));
        self.join();
    }

    /// Waits (a few seconds at most: a helper process may hold the pipes open) for the last lines to reach the log.
    fn join(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.readers.iter().any(|reader| !reader.is_finished()) && Instant::now() < deadline {
            sleep(0.05);
        }
        for reader in self.readers.drain(..) {
            reader.abort();
        }
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
        if Instant::now() >= deadline || interrupted().is_err() {
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

/// What a build is for (RELEASING.md → "Channels"): `dev`, the default, is a laptop's, a PR's or a manual run's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Channel {
    Release,
    Nightly,
    #[default]
    Dev,
}

impl Channel {
    pub fn parse(name: &str) -> Result<Channel> {
        match name {
            "release" => Ok(Channel::Release),
            "nightly" => Ok(Channel::Nightly),
            "dev" => Ok(Channel::Dev),
            _ => Err(format!("--channel release|nightly|dev, not {name}")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Channel::Release => "release",
            Channel::Nightly => "nightly",
            Channel::Dev => "dev",
        }
    }
}

/// What a release or a nightly adds to tauri.conf.json: signed update artifacts; a nightly also its version, and on
/// macOS a bundle version without its `-nightly.RUN` (CFBundleVersion is at most three numbers;
/// CFBundleShortVersionString keeps the whole version). Which feed each reads is the app's to say, by the channel it
/// was built for (core `updater::feed`): a nightly the nightly's, a release the releases' (RELEASING.md → "Updates").
pub fn channel_config(channel: Channel, nightly_version: &str) -> Option<String> {
    let updates = serde_json::json!({ "createUpdaterArtifacts": true });
    let config = match channel {
        Channel::Release => serde_json::json!({ "bundle": updates }),
        Channel::Nightly => serde_json::json!({
            "version": nightly_version,
            "bundle": {
                "createUpdaterArtifacts": true,
                "macOS": { "bundleVersion": numeric_version(nightly_version) },
            },
        }),
        Channel::Dev => return None,
    };
    Some(config.to_string())
}

/// A version's numbers, `X.Y.Z`, without a pre-release or build (`0.4.3-nightly.17` → `0.4.3`).
pub fn numeric_version(version: &str) -> &str {
    version.split(['-', '+']).next().unwrap_or_default()
}

/// A nightly's version: the last release's (`released`) with its patch + 1, then `-nightly.RUN` (0.4.2, run 17 →
/// 0.4.3-nightly.17): above that release, below the next, and higher every run (`RUN` is a number, compared as one), as
/// the updater needs: it offers only a higher version on the nightly's feed.
pub fn nightly_version(released: &str, run: &str) -> Result<String> {
    let numbers: std::result::Result<Vec<u64>, _> = numeric_version(released).split('.').map(str::parse).collect();
    let (Ok([major, minor, patch]), Ok(run)) = (numbers.as_deref(), run.parse::<u64>()) else {
        return Err(format!("no nightly version from release {released:?} and run {run:?}"));
    };
    Ok(format!("{major}.{minor}.{}-nightly.{run}", patch + 1))
}

/// The version CI builds and publishes the nightly as: from .release-please-manifest.json and `GITHUB_RUN_NUMBER`.
pub fn ci_nightly_version() -> Result<String> {
    let path = repo().join(".release-please-manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_slice(&read_bytes(&path)?).map_err(|e| format!("{}: {e}", path.display()))?;
    let run = std::env::var("GITHUB_RUN_NUMBER").map_err(|_| "a nightly is CI's: GITHUB_RUN_NUMBER is not set")?;
    nightly_version(manifest["."].as_str().unwrap_or_default(), &run)
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
    /// What the build is for ([`channel_config`]).
    pub channel: Channel,
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

    /// The command: the CLI's script run by Node (what `npx tauri` runs), for its channel; only a release or a nightly
    /// sees the updater's key, and the app's build script is told the channel (and a nightly's commit and date).
    pub fn command(&self) -> Result<Cmd> {
        let mut cmd = Cmd::new("node").arg(repo().join("node_modules/@tauri-apps/cli/tauri.js")).args(self.args());
        if self.channel == Channel::Dev {
            cmd = cmd.env_remove("TAURI_SIGNING_PRIVATE_KEY").env_remove("TAURI_SIGNING_PRIVATE_KEY_PASSWORD");
        }
        // A nightly's version, short commit and that commit's date (`YYYY-MM-DD`).
        let (version, commit, date) = match self.channel {
            Channel::Nightly => {
                let head = Cmd::new("git").args(["log", "-1", "--abbrev=7", "--format=%h %cs"]).stdout()?;
                let (commit, date) = head.trim().split_once(' ').ok_or("git log: no commit")?;
                (ci_nightly_version()?, commit.to_owned(), date.to_owned())
            }
            _ => Default::default(),
        };
        if let Some(config) = channel_config(self.channel, &version) {
            cmd = cmd.args(["--config", config.as_str()]);
        }
        Ok(cmd
            .env("SIDEVOICE_BUILD_CHANNEL", self.channel.name())
            .env("SIDEVOICE_BUILD_COMMIT", commit)
            .env("SIDEVOICE_BUILD_DATE", date))
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
        Files::ensure_private_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(SecretDir { path })
    }

    /// A new file in it holding `bytes`, created readable and writable by this user only (0600).
    pub fn file(&self, name: &str, bytes: &[u8]) -> Result<PathBuf> {
        let path = self.path.join(name);
        Files::create_private(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
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

/// Read an existing secret through the shared checked handle, without putting its contents in diagnostics.
pub fn read_secret(path: &Path) -> Result<Vec<u8>> {
    Files::read_private(path, 1024 * 1024)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .ok_or_else(|| format!("{}: secret file is missing", path.display()))
}

/// While it lives, files this process and its children create are private to this user (umask 077), whatever the
/// person's umask: Tauri's bundler writes the certificate it decodes with plain file creation.
pub struct PrivateFiles {
    #[cfg(unix)]
    before: rustix::fs::Mode,
}

impl PrivateFiles {
    pub fn start() -> PrivateFiles {
        #[cfg(unix)]
        {
            PrivateFiles { before: rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077)) }
        }
        #[cfg(not(unix))]
        PrivateFiles {}
    }
}

impl Drop for PrivateFiles {
    fn drop(&mut self) {
        #[cfg(unix)]
        rustix::process::umask(self.before);
    }
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
    getrandom::fill(&mut buffer).map_err(|e| e.to_string())?;
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

    fn fixture(mode: &str) -> Cmd {
        Cmd::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "--nocapture", "util::tests::child_fixture"])
            .env("SIDEVOICE_XTASK_FIXTURE", mode)
    }

    // Native fixtures exercise the same command interface on Linux, macOS and Windows.
    #[test]
    #[ignore = "child fixture"]
    fn child_fixture() {
        match std::env::var("SIDEVOICE_XTASK_FIXTURE").unwrap().as_str() {
            "output" => {
                std::io::stdout().write_all(&vec![b'o'; 128 * 1024]).unwrap();
                std::io::stderr().write_all(&vec![b'e'; 128 * 1024]).unwrap();
                println!("\nfixture-out");
                eprintln!("fixture-err");
            }
            "input" => {
                let text = std::io::read_to_string(std::io::stdin()).unwrap();
                println!("input={text}");
            }
            "sleep" => sleep(30.0),
            "orphan" => {
                // Deliberately leave a descendant holding both output pipes; the outer command owns cleanup.
                #[allow(clippy::zombie_processes)]
                let child = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--ignored", "--exact", "--nocapture", "util::tests::child_fixture"])
                    .env("SIDEVOICE_XTASK_FIXTURE", "sleep")
                    .spawn()
                    .unwrap();
                println!("descendant={}", child.id());
            }
            "interrupt" => {
                on_interrupt().unwrap();
                // Keep the process-global interruption flag in this subprocess, isolated from sibling tests.
                let dir = SecretDir::new(&format!("xtask-interrupt-{}", std::process::id())).unwrap();
                dir.file("key", b"fixture-secret").unwrap();
                println!("secret-dir={}", dir.path.display());
                if std::env::var_os("SIDEVOICE_XTASK_WAIT_FOR_SIGNAL").is_none() {
                    thread::spawn(|| {
                        sleep(0.2);
                        INTERRUPTED.store(true, Ordering::SeqCst);
                    });
                }
                assert_eq!(fixture("sleep").run().unwrap_err(), "interrupted");
                assert_eq!(fixture("output").run().unwrap_err(), "interrupted");
                drop(dir);
                println!("unwound");
            }
            _ => panic!("unknown fixture"),
        }
    }

    #[test]
    fn command_output_and_failure_diagnostics_keep_their_contract() {
        let (ok, text) = fixture("output").attempt().unwrap();
        assert!(ok);
        assert!(text.contains(&"o".repeat(128 * 1024)) && text.contains(&"e".repeat(128 * 1024)));
        assert!(text.find("fixture-out").unwrap() < text.find("fixture-err").unwrap());
        let err = Cmd::new("sidevoice-xtask-missing-fixture").secret("fixture-secret").run().unwrap_err();
        assert!(err.contains("***") && !err.contains("fixture-secret"));
    }

    #[test]
    fn piped_input_closes_and_background_logs_keep_both_streams() {
        let dir = tempfile::tempdir().unwrap();
        let piped = dir.path().join("input");
        fixture("input").pipe(b"fixture-input", File::create(&piped).unwrap()).unwrap();
        assert!(read(&piped).contains("input=fixture-input"));
        for timed in [false, true] {
            let log = dir.path().join("log");
            let mut child =
                if timed { fixture("output").spawn_timed(&log) } else { fixture("output").spawn(&log) }.unwrap();
            assert!(child.ended(10).unwrap().success());
            child.stop();
            let text = read(&log);
            assert!(text.contains("fixture-out") && text.contains("fixture-err"));
            if timed {
                assert!(text.lines().all(|line| line.split_once(' ').unwrap().0.parse::<f64>().is_ok()));
            }
        }
    }

    fn assert_gone(pid: u32) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Processes::identity(pid).is_some() && Instant::now() < deadline {
            sleep(0.02);
        }
        assert!(Processes::identity(pid).is_none(), "owned descendant stayed alive");
    }

    #[test]
    fn an_exited_leaders_descendant_cannot_hold_capture_or_background_open() {
        let start = Instant::now();
        let text = fixture("orphan").output().unwrap();
        assert!(start.elapsed() < Duration::from_secs(15));
        let pid = text.lines().find_map(|line| line.strip_prefix("descendant=")).unwrap().parse().unwrap();
        assert_gone(pid);
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        let mut child = fixture("orphan").spawn(&log).unwrap();
        assert!(child.ended(10).unwrap().success());
        drop(child);
        let pid = read(&log).lines().find_map(|line| line.strip_prefix("descendant=")).unwrap().parse().unwrap();
        assert_gone(pid);
    }

    #[test]
    fn interruption_stops_work_and_unwinds_secret_guards() {
        let text = fixture("interrupt").output().unwrap();
        assert!(text.contains("unwound"));
        let path = text.lines().find_map(|line| line.strip_prefix("secret-dir=")).unwrap();
        assert!(!Path::new(path).exists());
    }

    #[test]
    fn secret_files_use_shared_private_reads_and_are_removed_on_drop() {
        let parent = tempfile::tempdir().unwrap();
        let path = parent.path().join("secrets");
        {
            let dir = SecretDir::new(path.to_str().unwrap()).unwrap();
            let key = dir.file("key", b"fixture-secret").unwrap();
            assert_eq!(read_secret(&key).unwrap(), b"fixture-secret");
            use sidevoice_platform::fs::TestFiles;
            Files::let_others_read(&key);
            assert!(read_secret(&key).is_err(), "a readable-by-others key was accepted");
            Files::shut_others_out(&key);
            assert_eq!(read_secret(&key).unwrap(), b"fixture-secret");
            assert!(dir.file("key", b"replacement").is_err());
            let missing = read_secret(&dir.path.join("missing")).unwrap_err();
            assert!(!missing.contains("fixture-secret"));
        }
        assert!(!path.exists());
    }

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
    fn a_release_and_a_nightly_make_the_update_artifacts_and_a_nightly_has_its_own_version() {
        assert_eq!(channel_config(Channel::Release, "").unwrap(), r#"{"bundle":{"createUpdaterArtifacts":true}}"#);
        let nightly = channel_config(Channel::Nightly, "0.4.3-nightly.17").unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&nightly).unwrap(),
            serde_json::json!({
                "version": "0.4.3-nightly.17",
                "bundle": {"createUpdaterArtifacts": true, "macOS": {"bundleVersion": "0.4.3"}},
            }),
            "no feed: the app's, by its channel (core updater::feed)"
        );
        assert_eq!(channel_config(Channel::Dev, ""), None);
        assert_eq!(TauriBuild::default().channel, Channel::Dev, "a laptop's build is never a release");
    }

    #[test]
    fn a_nightly_s_version_is_above_the_last_release_and_grows_every_run() {
        assert_eq!(nightly_version("0.4.2", "17").unwrap(), "0.4.3-nightly.17");
        assert_eq!(nightly_version("0.3.0-rc.1", "5").unwrap(), "0.3.1-nightly.5");
        for (released, run) in [("0.4", "17"), ("0.4.x", "17"), ("0.4.2", ""), ("0.4.2", "a"), ("0.4.2", "-1")] {
            assert!(nightly_version(released, run).is_err(), "{released} {run}");
        }
    }

    /// As tauri-plugin-updater compares them (semver's order): it offers only a higher version than the running one.
    #[test]
    fn every_nightly_sorts_above_the_one_before_it_and_between_the_releases_around_it() {
        let v = |s: &str| semver::Version::parse(s).unwrap();
        let nightly = |released: &str, run: &str| v(&nightly_version(released, run).unwrap());
        // The run is compared as a number, not as text.
        assert!(nightly("0.4.2", "9") < nightly("0.4.2", "17"));
        assert!(nightly("0.4.2", "17") < nightly("0.4.2", "100"));
        // A release in between (the manifest moves to it): the nightlies after it sort above those before.
        assert!(nightly("0.4.2", "17") < nightly("0.5.0", "18"));
        assert!(nightly("0.3.0-rc.1", "5") < nightly("0.3.0", "6"));
        // Above the release it is built after, below whichever comes next (a patch, a minor, a major).
        for next in ["0.4.3", "0.5.0", "1.0.0"] {
            assert!(v("0.4.2") < nightly("0.4.2", "17") && nightly("0.4.2", "17") < v(next), "{next}");
        }
    }

    #[test]
    fn a_bundle_s_numeric_version_drops_the_pre_release() {
        assert_eq!(numeric_version("0.4.3-nightly.17"), "0.4.3");
        assert_eq!(numeric_version("0.3.0-rc.1"), "0.3.0");
        assert_eq!(numeric_version("0.4.2"), "0.4.2");
        // macOS: CFBundleVersion is one to three numbers, dot-separated.
        let nightly = nightly_version("0.4.2", "17").unwrap();
        let parts: Vec<&str> = numeric_version(&nightly).split('.').collect();
        assert!(parts.len() <= 3 && parts.iter().all(|p| p.parse::<u64>().is_ok()), "{parts:?}");
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
