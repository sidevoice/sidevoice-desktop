//! Reset admission and authenticated helper handoff. No deletion policy lives here.
//!
//! Two non-inherited locks avoid a platform-specific lock transfer: the parent keeps `admission.lock`
//! while the helper takes `reset.lock`. Startup must acquire both, keeping them through initialization.
//! Only an authenticated, committed helper may wait for the parent's admission lock and continue.
//! Where a running process's open files cannot be removed (Windows), the helper also waits for the parent itself to
//! have terminated, through a hold on it taken while the handshake proves it alive.

use serde::{Deserialize, Serialize};
use sidevoice_platform::{
    fs::{Access, Create, Directory, Files, Fs, HeldDirectory},
    ipc::{Address, Channel, Channels, Endpoint, Stream},
    process::{Child, Process, Processes},
};
use std::{
    fs::{File, TryLockError},
    io::{self, Read},
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    runtime::Runtime,
};

#[cfg_attr(target_os = "macos", path = "reset_handoff/macos.rs")]
#[cfg_attr(target_os = "windows", path = "reset_handoff/windows.rs")]
#[cfg_attr(target_os = "linux", path = "reset_handoff/linux.rs")]
mod imp;
use imp::Native;

/// What differs per OS: whether the parent's own exit must be awaited, and the hold that observes it.
trait Os {
    /// Whether a running process's open files are kept from removal (Windows). Where they are, the helper's cleanup
    /// also waits for the parent to have terminated, every handle of it closed; the lock and EOF alone do not say so.
    const OPEN_FILES_BLOCK_REMOVAL: bool = false;

    /// Whether the installed app is checked as a Sidevoice program before the helper or the relaunch starts it. Where
    /// it is not, the handoff keeps the launch it shipped with: the executable as the OS names it, unchecked.
    const PROGRAM_CHECKED: bool = true;

    /// A hold on one live process that can later tell whether it has terminated.
    type Held: Send;

    /// Hold the live process `pid`. Called only while the handshake proves the parent alive, so a reused PID is never
    /// the one held.
    fn hold(_pid: u32) -> io::Result<Self::Held> {
        Err(io::ErrorKind::Unsupported.into())
    }

    /// Whether the held process has terminated. Never blocks.
    fn terminated(_held: &Self::Held) -> io::Result<bool> {
        Ok(false)
    }
}

const HANDSHAKE: Duration = Duration::from_secs(5);
const EXIT: Duration = Duration::from_secs(60);
const LIMIT: usize = 16 * 1024;
const COMMIT: u8 = 1;
const CANCEL: u8 = 2;

/// Only orchestration data, never caller-selected deletion paths. The PID is an observation for browser
/// discovery, not authentication or proof of exit.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResetPlan {
    pub parent_pid: u32,
    pub uninstall: bool,
    pub initial: serde_json::Value,
}

/// Private control state outside every reset deletion root, the same on every host: a sibling of the standard
/// data root (`…/dev.sidevoice.desktop-reset`), so no cleanup of that root can remove it. Never follows
/// SIDEVOICE_DATA_DIR.
pub fn state_dir() -> io::Result<PathBuf> {
    sidevoice_platform::data_dir::standard()
        .and_then(|path| path.parent().map(|parent| parent.join("dev.sidevoice.desktop-reset")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "reset state directory unavailable"))
}

/// The reset handoff in its control directory. The protocol is identical on every host: the shared checked
/// filesystem, IPC and child primitives supply what differs.
pub struct Handoff {
    root: PathBuf,
}

impl Handoff {
    /// The handoff in [`state_dir`].
    pub fn new() -> io::Result<Self> {
        Ok(Self { root: state_dir()? })
    }

    /// Exclusive admission before any transaction side effects. Busy is `WouldBlock`.
    pub fn admit(&self) -> io::Result<Admission> {
        admit_at(&self.root)
    }

    /// Wait at most `timeout`, or until cancelled, before initializing ANY application writers/plugins.
    /// Hold the returned admission through initialization; dropping it on failure releases both locks.
    pub fn await_idle(&self, timeout: Duration, cancelled: &AtomicBool) -> io::Result<Admission> {
        await_at(&self.root, timeout, cancelled)
    }

    /// Helper-only: read the capability from inherited stdin and receive a bounded, authenticated plan.
    /// No CLI PID, descriptor number, environment secret or on-disk secret is accepted as authorization.
    pub fn receive(&self) -> io::Result<Helper> {
        receive_at(&self.root, HANDSHAKE)
    }
}

/// The installed app, as Tauri resolves its own binary (Linux: the AppImage itself rather than its temporary mount,
/// which goes away with the app; Tauri takes it from `APPIMAGE`, an environment the app trusts as it trusts `HOME`),
/// checked as a Sidevoice program (`trusted_program`: this user's or the system's, and nobody else may change it or the
/// folders above it). Where the handoff keeps its shipped launch (`PROGRAM_CHECKED` false), the executable as the OS
/// names it. The helper and the relaunch start only this; neither the page nor the plan names a program.
pub fn installed_program() -> Option<PathBuf> {
    if !Native::PROGRAM_CHECKED {
        return std::env::current_exe().ok();
    }
    let program = tauri::process::current_binary(&tauri::Env::default()).ok()?;
    Files::trusted_program(&program).ok()
}

/// Launch `program` (this app's own executable, as the caller resolved and checked it) in helper mode, detached and
/// windowless, authenticate it and commit its plan. The returned guard must remain alive until process exit.
/// Dropping it cancels and reaps the helper before releasing admission. This blocking operation belongs on the
/// blocking pool, never inside a Tokio runtime.
pub fn launch(admission: Admission, plan: ResetPlan, program: &Path) -> io::Result<Pending> {
    let mut command = tokio::process::Command::new(program);
    command.arg("--sidevoice-reset-helper");
    launch_command(admission, plan, command, HANDSHAKE)
}

/// Start `program` (the installed app, as the caller resolved and checked it) once cleanup is over: detached from the
/// helper, windowless, with none of the helper's standard streams, and `marker=1` in its environment. A started
/// process is not proof that the new app finished startup; that app's setup says so.
pub fn relaunch(program: &Path, marker: &str) -> io::Result<()> {
    let runtime = runtime()?;
    let mut command = tokio::process::Command::new(program);
    command.env(marker, "1").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    runtime.block_on(Processes::spawn_detached(command)).map(drop)
}

/// Finish startup and release the barrier. Transaction journal acknowledgement stays with its caller.
pub fn finish_startup(admission: Admission) {
    drop(admission);
}

/// Holds checked ancestry as well as file locks. Files are never unlinked (which would split the lock).
pub struct Admission {
    root: PathBuf,
    gate: Lock,
    reset: Lock,
    _directory: HeldDirectory,
}

fn directory(root: &Path) -> io::Result<HeldDirectory> {
    Files::ensure_private_dir(root)?;
    HeldDirectory::open(root, Access::Private)
}

struct Lock(File);

impl Drop for Lock {
    fn drop(&mut self) {
        // Explicit unlock also releases a transient fork's copy before its close-on-exec runs.
        let _ = self.0.unlock();
    }
}

fn lock(root: &Path, name: &str) -> io::Result<Lock> {
    let file = Files::open_private(&root.join(name), Create::ReadWrite)?;
    Files::check_private(&file)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "reset lock is not a regular file"));
    }
    match file.try_lock() {
        Ok(()) => Ok(Lock(file)),
        Err(TryLockError::WouldBlock) => Err(io::ErrorKind::WouldBlock.into()),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

fn admit_at(root: &Path) -> io::Result<Admission> {
    let directory = directory(root)?;
    let gate = lock(root, "admission.lock")?;
    let reset = lock(root, "reset.lock")?;
    Ok(Admission { root: root.to_owned(), gate, reset, _directory: directory })
}

fn await_at(root: &Path, timeout: Duration, cancelled: &AtomicBool) -> io::Result<Admission> {
    let deadline = Instant::now() + timeout;
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        match admit_at(root) {
            Ok(guard) => return Ok(guard),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => (),
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn runtime() -> io::Result<Runtime> {
    tokio::runtime::Builder::new_current_thread().enable_all().build()
}

/// Owns the launched child, live control channel and parent admission. Explicit cancellation is Drop;
/// normal success ends with process exit (which closes the channel and gate together).
pub struct Pending {
    child: Child,
    stream: Option<Stream>,
    runtime: Runtime,
    _gate: Lock,
    _directory: HeldDirectory,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.runtime.block_on(async {
            if let Some(stream) = &mut self.stream {
                let _ = tokio::time::timeout(HANDSHAKE, stream.write_all(&[CANCEL])).await;
            }
            // Keep admission until cancellation/termination, including on a failed handshake.
            self.child.kill_tree().await;
        });
    }
}

fn launch_command(
    admission: Admission,
    plan: ResetPlan,
    mut command: tokio::process::Command,
    timeout: Duration,
) -> io::Result<Pending> {
    let payload = serde_json::to_vec(&plan)?;
    if payload.len() > LIMIT {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let runtime = runtime()?;
    let mut secret = [0; 32];
    getrandom::fill(&mut secret).map_err(|error| io::Error::other(error.to_string()))?;
    let endpoint = Endpoint::new(&admission.root, "reset");
    let listener = {
        let _entered = runtime.enter();
        Channels::listen(&endpoint)?
    };
    command.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    let child = runtime.block_on(Processes::spawn_detached(command))?;
    let Admission { gate, reset, _directory, .. } = admission;
    let mut pending = Pending { child, stream: None, runtime, _gate: gate, _directory };
    // No startup/reset can pass the parent gate while the helper takes its own independent lock.
    drop(reset);
    let stream = pending.runtime.block_on(async {
        tokio::time::timeout(timeout, async {
            let mut stdin = pending.child.stdin().ok_or_else(|| io::Error::other("missing helper stdin"))?;
            stdin.write_all(&secret).await?;
            drop(stdin);
            let mut stream = listener.accept().await?;
            let mut proof = [0; 32];
            stream.read_exact(&mut proof).await?;
            if proof != secret {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "wrong reset capability"));
            }
            stream.write_u32(payload.len() as u32).await?;
            stream.write_all(&payload).await?;
            if stream.read_u8().await? != COMMIT {
                return Err(io::ErrorKind::InvalidData.into());
            }
            stream.write_all(&[COMMIT]).await?;
            Ok(stream)
        })
        .await?
    })?;
    pending.stream = Some(stream);
    Ok(pending)
}

/// Authenticated helper plan, with the cleanup lock already held. Before acknowledging readiness the
/// caller may capture browser-process observations; it must perform no cleanup until `await_exit` succeeds.
pub struct Helper {
    pub plan: ResetPlan,
    /// The parent, held while the handshake proved it alive, where its own exit must be awaited.
    parent: Option<<Native as Os>::Held>,
    root: PathBuf,
    reset: Lock,
    directory: HeldDirectory,
    stream: Stream,
    runtime: Runtime,
}

fn receive_at(root: &Path, timeout: Duration) -> io::Result<Helper> {
    // A blocking stdin read must not keep a manually invoked helper alive indefinitely. This thread exists
    // only in helper mode; on timeout main returns and the process exits, including the blocked reader.
    let (sent, received) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut secret = [0; 32];
        let result = std::io::stdin().read_exact(&mut secret).map(|_| secret);
        let _ = sent.send(result);
    });
    let secret = received.recv_timeout(timeout).map_err(|_| io::ErrorKind::TimedOut)??;
    let directory = directory(root)?;
    let runtime = runtime()?;
    let (stream, plan) = runtime.block_on(async {
        tokio::time::timeout(timeout, async {
            let mut stream = Channels::connect(&Endpoint::new(root, "reset")).await?;
            stream.write_all(&secret).await?;
            let size = stream.read_u32().await? as usize;
            if size > LIMIT {
                return Err(io::Error::from(io::ErrorKind::InvalidData));
            }
            let mut bytes = vec![0; size];
            stream.read_exact(&mut bytes).await?;
            let plan: ResetPlan = serde_json::from_slice(&bytes)?;
            if plan.parent_pid <= 1 || !plan.initial.get("failed").is_some_and(serde_json::Value::is_array) {
                return Err(io::ErrorKind::InvalidData.into());
            }
            Ok((stream, plan))
        })
        .await?
    })?;
    // Held from here, the parent's termination can later be observed by handle rather than by PID. The plan may have
    // been read from the buffer after the parent died and its PID was reused: then the process held is another one.
    // That cannot authorize cleanup: a parent that died here never sees this helper's readiness, so never sends the
    // COMMIT that `wait_for_exit` requires before the lock and the termination. A hold that fails refuses.
    let parent = if Native::OPEN_FILES_BLOCK_REMOVAL { Some(Native::hold(plan.parent_pid)?) } else { None };
    let reset = lock(root, "reset.lock")?;
    Ok(Helper { plan, parent, root: root.to_owned(), reset, directory, stream, runtime })
}

impl Helper {
    /// Acknowledge readiness, require explicit commit then channel EOF, and acquire the original parent's
    /// gate. EOF alone never authorizes cleanup. Holding both locks proves admission has passed to us;
    /// numeric PID reuse cannot authorize this transition. Where open files block removal, the held parent must
    /// also have terminated. Timeout/cancel fails without cleanup.
    pub fn await_exit(self) -> io::Result<Admission> {
        self.wait_for_exit(EXIT)
    }

    fn wait_for_exit(mut self, timeout: Duration) -> io::Result<Admission> {
        let gate = self.runtime.block_on(async {
            tokio::time::timeout(timeout, async {
                self.stream.write_all(&[COMMIT]).await?;
                if self.stream.read_u8().await? != COMMIT {
                    return Err(io::Error::from(io::ErrorKind::Interrupted));
                }
                let mut byte = [0];
                if self.stream.read(&mut byte).await? != 0 {
                    return Err(io::Error::from(io::ErrorKind::Interrupted));
                }
                let gate = loop {
                    match lock(&self.root, "admission.lock") {
                        Ok(gate) => break gate,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => (),
                        Err(error) => return Err(error),
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                };
                // The gate is released as the parent's handles close, not after the last of them: its other files may
                // still be open. Within the same deadline, wait for the process itself.
                if let Some(parent) = &self.parent {
                    while !Native::terminated(parent)? {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                }
                Ok(gate)
            })
            .await?
        })?;
        Ok(Admission { root: self.root, gate, reset: self.reset, _directory: self.directory })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(root: &Path) -> Handoff {
        Handoff { root: root.to_owned() }
    }

    fn scratch() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("reset");
        Files::create_private_dir(&root).unwrap();
        (temp, root)
    }

    fn plan() -> ResetPlan {
        ResetPlan { parent_pid: std::process::id(), uninstall: false, initial: json!({"failed": [], "agents": {}}) }
    }

    fn command(root: &Path, mode: &str) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        let name = module_path!().split_once("::").unwrap().1;
        command.args(["--exact", &format!("{name}::helper_process"), "--nocapture"]);
        command.env("SIDEVOICE_TEST_RESET_ROOT", root).env("SIDEVOICE_TEST_RESET_MODE", mode);
        command
    }

    fn wait_marker(root: &Path, name: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !root.join(name).exists() {
            assert!(Instant::now() < deadline, "missing {name}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn mark(root: &Path, name: &str) {
        Files::write_private(&root.join(name), b"ok").unwrap();
    }

    // Native test executable in helper/parent modes: the same protocol runs on all three OSes.
    #[test]
    fn helper_process() {
        let Some(mode) = std::env::var_os("SIDEVOICE_TEST_RESET_MODE") else {
            return;
        };
        let root = PathBuf::from(std::env::var_os("SIDEVOICE_TEST_RESET_ROOT").unwrap());
        match mode.to_str().unwrap() {
            "parent" => {
                let mut helper = command(&root, "helper");
                // Fixture-only: the helper compares what it receives with this process's real PID.
                helper.env("SIDEVOICE_TEST_PARENT_PID", std::process::id().to_string());
                let pending = launch_command(at(&root).admit().unwrap(), plan(), helper, HANDSHAKE).unwrap();
                mark(&root, "ready");
                wait_marker(&root, "exit");
                // Real process exit closes the live channel and parent gate without running cancellation Drop.
                std::mem::forget(pending);
                std::process::exit(0);
            }
            "helper" => {
                let helper = at(&root).receive().unwrap();
                // The plan arrives as the parent committed it, with the parent's actual PID.
                let parent_pid: u32 = std::env::var("SIDEVOICE_TEST_PARENT_PID").unwrap().parse().unwrap();
                let sent = plan();
                assert_ne!(parent_pid, std::process::id());
                assert_eq!(
                    (helper.plan.parent_pid, helper.plan.uninstall, &helper.plan.initial),
                    (parent_pid, sent.uninstall, &sent.initial)
                );
                let guard = helper.await_exit().unwrap();
                mark(&root, "cleanup");
                wait_marker(&root, "finish");
                drop(guard);
                mark(&root, "done");
            }
            "timeout" => {
                let helper = at(&root).receive().unwrap();
                assert_eq!(
                    helper.wait_for_exit(Duration::from_millis(150)).err().unwrap().kind(),
                    io::ErrorKind::TimedOut
                );
                mark(&root, "timed-out");
            }
            "refuse" => {
                let helper = at(&root).receive().unwrap();
                assert!(helper.await_exit().is_err());
                mark(&root, "refused");
            }
            "wrong" => {
                let runtime = runtime().unwrap();
                runtime.block_on(async {
                    let mut stream = Channels::connect(&Endpoint::new(&root, "reset")).await.unwrap();
                    stream.write_all(&[0; 32]).await.unwrap();
                });
            }
            "silent" => std::thread::sleep(Duration::from_secs(10)),
            "direct" => {
                assert!(receive_at(&root, Duration::from_millis(100)).is_err());
                mark(&root, "refused");
            }
            other => panic!("unknown test mode {other}"),
        }
    }

    #[test]
    fn every_host_keeps_control_state_beside_the_standard_data_root() {
        let root = Handoff::new().unwrap().root;
        assert_eq!(root, state_dir().unwrap());
        assert!(root.is_absolute());
        let data = sidevoice_platform::data_dir::standard().unwrap();
        assert_eq!(root.parent(), data.parent());
        assert!(!root.starts_with(&data), "cleanup must not remove the handoff barrier");
    }

    #[test]
    fn production_launch_fails_closed_when_the_executable_does_not_authenticate() {
        // This test executable rejects the helper flag and exits: no capability ever comes back.
        let (_temp, root) = scratch();
        let started = Instant::now();
        let refused = launch(at(&root).admit().unwrap(), plan(), &std::env::current_exe().unwrap()).err().unwrap();
        // It may exit before the capability is written (BrokenPipe) or after (no reply: TimedOut); both fail closed.
        assert!(matches!(refused.kind(), io::ErrorKind::TimedOut | io::ErrorKind::BrokenPipe), "{refused}");
        assert!(started.elapsed() < HANDSHAKE + Duration::from_secs(5), "bounded by the handshake deadline");
        assert!(at(&root).admit().is_ok(), "a failed launch releases admission");
    }

    #[test]
    fn admission_and_startup_are_exclusive_until_finish() {
        fn send<T: Send>() {}
        send::<Admission>(); // Tauri's setup closure.
        send::<Pending>(); // Tauri managed state behind a mutex.
        let (_temp, root) = scratch();
        let guard = at(&root).admit().unwrap();
        assert_eq!(at(&root).admit().err().unwrap().kind(), io::ErrorKind::WouldBlock);
        assert_eq!(
            at(&root).await_idle(Duration::ZERO, &AtomicBool::new(false)).err().unwrap().kind(),
            io::ErrorKind::TimedOut
        );
        finish_startup(guard);
        let guard = at(&root).await_idle(HANDSHAKE, &AtomicBool::new(false)).unwrap();
        assert!(at(&root).admit().is_err());
        drop(guard);
        assert!(at(&root).admit().is_ok());
    }

    #[test]
    fn startup_can_be_cancelled_without_admission() {
        let (_temp, root) = scratch();
        assert_eq!(
            at(&root).await_idle(HANDSHAKE, &AtomicBool::new(true)).err().unwrap().kind(),
            io::ErrorKind::Interrupted
        );
        assert!(at(&root).admit().is_ok());
    }

    #[test]
    fn second_launch_is_blocked_across_parent_exit_and_helper_cleanup() {
        let (_temp, root) = scratch();
        let runtime = runtime().unwrap();
        let mut parent = runtime.block_on(Processes::spawn_detached(command(&root, "parent"))).unwrap();
        wait_marker(&root, "ready");
        assert_eq!(at(&root).admit().err().unwrap().kind(), io::ErrorKind::WouldBlock);
        assert!(!root.join("cleanup").exists(), "cleanup cannot start while parent is alive");
        mark(&root, "exit");
        wait_marker(&root, "cleanup");
        assert_eq!(at(&root).admit().err().unwrap().kind(), io::ErrorKind::WouldBlock);
        mark(&root, "finish");
        wait_marker(&root, "done");
        assert!(at(&root).admit().is_ok());
        runtime.block_on(parent.kill_tree());
    }

    #[test]
    fn dropping_pending_cancels_and_reaps_before_unlocking() {
        let (_temp, root) = scratch();
        let pending = launch_command(at(&root).admit().unwrap(), plan(), command(&root, "refuse"), HANDSHAKE).unwrap();
        assert!(at(&root).admit().is_err());
        drop(pending);
        assert!(at(&root).admit().is_ok());
        assert!(!root.join("cleanup").exists());
    }

    #[test]
    fn helper_exit_deadline_releases_its_lock_but_parent_still_blocks_startup() {
        let (_temp, root) = scratch();
        let pending = launch_command(at(&root).admit().unwrap(), plan(), command(&root, "timeout"), HANDSHAKE).unwrap();
        wait_marker(&root, "timed-out");
        assert!(at(&root).admit().is_err());
        drop(pending);
        assert!(at(&root).admit().is_ok());
    }

    #[test]
    fn wrong_capability_and_unresponsive_helper_fail_closed_and_release_admission() {
        for mode in ["wrong", "silent"] {
            let (_temp, root) = scratch();
            assert!(launch_command(
                at(&root).admit().unwrap(),
                plan(),
                command(&root, mode),
                Duration::from_millis(300)
            )
            .is_err());
            assert!(at(&root).admit().is_ok());
        }
    }

    #[test]
    fn spawn_failure_and_oversized_plan_release_admission() {
        let (_temp, root) = scratch();
        assert!(launch_command(
            at(&root).admit().unwrap(),
            plan(),
            tokio::process::Command::new(root.join("missing")),
            HANDSHAKE
        )
        .is_err());
        let mut large = plan();
        large.initial = json!({"failed": [], "huge": "x".repeat(LIMIT)});
        assert_eq!(
            launch_command(at(&root).admit().unwrap(), large, command(&root, "helper"), HANDSHAKE)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(at(&root).admit().is_ok());
    }

    #[test]
    fn manual_helper_without_inherited_capability_times_out() {
        let (_temp, root) = scratch();
        let runtime = runtime().unwrap();
        let mut command = command(&root, "direct");
        // Keep stdin open without sending a capability, like an accidental terminal invocation.
        command.stdin(Stdio::piped());
        let mut child = runtime.block_on(Processes::spawn_detached(command)).unwrap();
        wait_marker(&root, "refused");
        runtime.block_on(child.kill_tree());
        assert!(at(&root).admit().is_ok());
    }

    #[test]
    fn pipe_eof_without_commit_never_authorizes_cleanup() {
        let (_temp, root) = scratch();
        let admission = at(&root).admit().unwrap();
        let runtime = runtime().unwrap();
        runtime.block_on(async {
            let listener = Channels::listen(&Endpoint::new(&root, "reset")).unwrap();
            let mut command = command(&root, "refuse");
            command.stdin(Stdio::piped());
            let mut child = Processes::spawn_detached(command).await.unwrap();
            child.stdin().unwrap().write_all(&[7; 32]).await.unwrap();
            drop(admission.reset);
            let mut stream = listener.accept().await.unwrap();
            let mut proof = [0; 32];
            stream.read_exact(&mut proof).await.unwrap();
            assert_eq!(proof, [7; 32]);
            let payload = serde_json::to_vec(&plan()).unwrap();
            stream.write_u32(payload.len() as u32).await.unwrap();
            stream.write_all(&payload).await.unwrap();
            assert_eq!(stream.read_u8().await.unwrap(), COMMIT);
            drop(stream); // parent crash BEFORE commit
            tokio::time::timeout(HANDSHAKE, child.wait()).await.unwrap().unwrap();
            child.kill_tree().await;
        });
        assert!(root.join("refused").exists());
        assert!(!root.join("cleanup").exists());
    }

    #[test]
    fn a_held_parent_is_terminated_only_once_it_has_exited() {
        let (_temp, root) = scratch();
        // This test executable in a mode that only sleeps: a live process to hold.
        let name = module_path!().split_once("::").unwrap().1;
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("{name}::helper_process")])
            .env("SIDEVOICE_TEST_RESET_ROOT", &root)
            .env("SIDEVOICE_TEST_RESET_MODE", "silent")
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        // A host that awaits the parent's exit holds a live process; elsewhere the handoff's own proof stands.
        let held = Native::hold(child.id());
        assert_eq!(held.is_ok(), Native::OPEN_FILES_BLOCK_REMOVAL);
        if let Ok(held) = &held {
            assert!(!Native::terminated(held).unwrap(), "a live process is not terminated");
        }
        child.kill().unwrap();
        child.wait().unwrap();
        if let Ok(held) = &held {
            assert!(Native::terminated(held).unwrap(), "an exited process is");
        }
    }

    #[test]
    fn a_relaunch_that_cannot_start_says_so() {
        let (_temp, root) = scratch();
        assert!(relaunch(&root.join("missing"), "SIDEVOICE_TEST_RELAUNCH").is_err());
    }

    #[test]
    fn lock_replacement_with_a_hardlink_is_refused() {
        let (_temp, root) = scratch();
        drop(at(&root).admit().unwrap());
        std::fs::hard_link(root.join("reset.lock"), root.join("alias")).unwrap();
        assert_eq!(at(&root).admit().err().unwrap().kind(), io::ErrorKind::PermissionDenied);
    }
}
