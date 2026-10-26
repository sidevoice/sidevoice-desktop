//! Browser storage owned by the desktop reset. The transaction owns admission and the launch barrier.

use sidevoice_platform::fs::{Access, Directory, HeldDirectory};
use sidevoice_platform::removal::Removal;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::{io, thread};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[cfg_attr(target_os = "macos", path = "webview_data/macos.rs")]
#[cfg_attr(target_os = "windows", path = "webview_data/windows.rs")]
#[cfg_attr(target_os = "linux", path = "webview_data/linux.rs")]
mod imp;
use imp::Os;

/// The app's bundle identifier (`tauri.conf.json`), which names its browser storage on every host.
const IDENTIFIER: &str = "dev.sidevoice.desktop";

/// Native-resolved allowlist, never populated from the page or a reset payload.
pub struct Locations(Vec<PathBuf>);

impl Locations {
    /// Exact roots; some also contain app settings, so remove them only during delete-all.
    pub fn paths(&self) -> &[PathBuf] {
        &self.0
    }
}

/// A pre-exit observation, retained by the helper while it waits for the app to exit.
pub struct Owner {
    locations: Locations,
    processes: Vec<(Pid, u64)>,
}

/// Proof that the observed writers have exited. Keep the reset/launch lock held while using this value.
pub struct Released(Locations);

/// Check launch-mode eligibility at reset admission, before confirmation or any destructive step.
/// macOS refuses (`Unsupported`) an app that is not its own responsible process.
pub fn preflight(owner: u32) -> io::Result<()> {
    Os::preflight(owner)
}

/// Resolve the runtime's roots natively, before exit; `home` is the account's home directory.
/// Callers never spell a native path. macOS keeps its existing WKWebView default store.
pub fn locations(home: &Path) -> io::Result<Locations> {
    Os::locations(home)
}

/// Snapshot the app and its browser writers before allowing the app to exit. The transaction
/// must already forbid new webviews and keep other app launches behind its held reset lock.
/// Windows also discovers runtimes using this user-data folder; macOS tracks responsible XPC services.
pub fn capture(owner: u32, locations: Locations) -> io::Result<Owner> {
    capture_with::<Os>(owner, locations)
}

/// Wait for the app and its observed browser processes, refreshing descendants and folder users
/// until `deadline`. Never kill unrelated browsers or treat a timeout as permission to delete.
/// The app must stop creating webviews before capture; the helper must retain the launch barrier
/// through removal. A fresh process must never initialize writers between release and deletion.
pub fn wait_released(owner: Owner, deadline: Instant) -> io::Result<Released> {
    wait_released_with::<Os>(owner, deadline)
}

/// Remove one exact allowlisted root only after browser release, using core's held-parent removal.
/// No directory is created or repaired; a missing root is success. Redirected roots are refused.
/// The caller keeps recovery records outside these roots and retains its reset/launch lock.
pub fn remove(released: &Released, path: &Path) -> io::Result<()> {
    if !released.0.paths().iter().any(|allowed| allowed == path) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "outside webview roots"));
    }
    // Opening the target first rejects root symlinks/junctions rather than unlinking them.
    let target = match HeldDirectory::open(path, Access::NotWritableByOthers) {
        Ok(target) => target,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let parent = path.parent().ok_or_else(Owner::busy)?;
    let name = path.file_name().ok_or_else(Owner::busy)?;
    let held = HeldDirectory::open(parent, Access::NotWritableByOthers)?;
    held.check_contents(&[name])?;
    // Windows denies deleting the target while the target's own held handle is open. The parent
    // remains held, and Removal repeats identity/ownership/mount checks before and during deletion.
    drop(target);
    held.remove_contents(&[name])
}

// What varies per OS, and nothing else. Browser cleanup only: no service stop, reset lock, journal,
// relaunch or app removal policy. Each `webview_data/<os>.rs` implements this private trait; the
// functions above hold the common code once and are the module's only interface.
trait Native {
    /// Launch-mode eligibility at reset admission. Hosts without such a constraint admit every launch.
    fn preflight(_owner: u32) -> io::Result<()> {
        Ok(())
    }

    /// Independently prove a tracked PID has exited. Snapshot omission is never exit evidence.
    /// Inspection failure refuses cleanup; even a reused live PID conservatively keeps the gate closed.
    fn exited(pid: u32) -> io::Result<bool>;

    /// The runtime's storage roots; `home` is the account's home directory.
    fn locations(_home: &Path) -> io::Result<Locations> {
        // Tauri's `app_local_data_dir` (the same `dirs` local data folder plus the identifier), which it
        // supplies to Wry. WebView2 uses it as its UDF; WebKitGTK uses it for BOTH base_data_directory
        // and base_cache_directory, with cookies at <path>/cookies.
        let local = dirs::data_local_dir()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no local data folder for this account"))?;
        Locations::new(vec![local.join(IDENTIFIER)])
    }

    /// Add browser writers to the pre-exit observation. `sysinfo` owns native process enumeration;
    /// selection is desktop policy.
    fn observe(system: &System, locations: &Locations, processes: &mut Vec<(Pid, u64)>) -> io::Result<()>;
}

fn capture_with<N: Native>(owner: u32, locations: Locations) -> io::Result<Owner> {
    let system = Owner::snapshot()?;
    let pid = Pid::from_u32(owner);
    let app = system.process(pid).ok_or_else(Owner::busy)?;
    if app.start_time() == 0 {
        return Err(Owner::busy());
    }
    let mut processes = vec![(pid, app.start_time())];
    N::observe(&system, &locations, &mut processes)?;
    Ok(Owner { locations, processes })
}

fn wait_released_with<N: Native>(mut owner: Owner, deadline: Instant) -> io::Result<Released> {
    loop {
        let system = Owner::snapshot()?;
        N::observe(&system, &owner.locations, &mut owner.processes)?;
        let mut all_exited = true;
        for (pid, _) in &owner.processes {
            // Query every tracked PID independently of metadata/name enumeration. A failed
            // inspection is an error, never an absent process; start-time mismatch alone is
            // likewise insufficient. PID reuse may delay cleanup but cannot release a live writer.
            all_exited &= N::exited(pid.as_u32())?;
        }
        if all_exited {
            return Ok(Released(owner.locations));
        }
        if Instant::now() >= deadline {
            return Err(Owner::busy());
        }
        thread::sleep(Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())));
    }
}

impl Locations {
    fn new(paths: Vec<PathBuf>) -> io::Result<Locations> {
        if paths.iter().any(|path| {
            !path.is_absolute()
                || path.parent().is_none()
                || path.components().any(|part| matches!(part, std::path::Component::ParentDir))
        }) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid native webview root"));
        }
        Ok(Locations(paths))
    }
}

impl Owner {
    fn busy() -> io::Error {
        io::Error::new(io::ErrorKind::WouldBlock, "webview writers have not been released")
    }

    fn snapshot() -> io::Result<System> {
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always).with_user(UpdateKind::Always),
        );
        // Enumeration failure must never be mistaken for an empty machine.
        system.process(Pid::from_u32(std::process::id())).ok_or_else(Owner::busy)?;
        Ok(system)
    }

    // Retain start times so a recycled PID cannot authorize deletion or cause an unrelated process to be killed.
    // Include intermediate descendants too: a renderer can start its own children. Exclude this reset helper.
    fn descendants(system: &System, processes: &mut Vec<(Pid, u64)>) {
        loop {
            let added: Vec<_> = system
                .processes()
                .iter()
                .filter(|(pid, process)| {
                    pid.as_u32() != std::process::id()
                        && !processes.contains(&(**pid, process.start_time()))
                        && process.parent().is_some_and(|parent| {
                            processes.iter().any(|(pid, start)| {
                                *pid == parent && system.process(parent).is_some_and(|p| p.start_time() == *start)
                            })
                        })
                })
                .map(|(pid, process)| (*pid, process.start_time()))
                .collect();
            if added.is_empty() {
                break;
            }
            processes.extend(added);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A native test subprocess stands in for a browser, so exercise the shared lifetime contract
    // without pretending that its executable/arguments identify a real WebView2 runtime.
    pub(super) struct Fixture;
    impl Native for Fixture {
        fn exited(pid: u32) -> io::Result<bool> {
            Os::exited(pid)
        }
        fn observe(system: &System, _locations: &Locations, processes: &mut Vec<(Pid, u64)>) -> io::Result<()> {
            Owner::descendants(system, processes);
            Ok(())
        }
    }

    #[test]
    fn live_owner_prevents_removal_even_without_a_browser() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Locations::new(vec![temp.path().join(IDENTIFIER)]).unwrap();
        let owner = capture_with::<Fixture>(std::process::id(), paths).unwrap();
        assert_eq!(
            wait_released_with::<Fixture>(owner, Instant::now()).err().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn missing_snapshot_and_inspection_failure_do_not_authorize_removal() {
        struct FailedInspection;
        impl Native for FailedInspection {
            fn observe(_: &System, _: &Locations, _: &mut Vec<(Pid, u64)>) -> io::Result<()> {
                Ok(())
            }
            fn exited(_: u32) -> io::Result<bool> {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let selected = temp.path().join(IDENTIFIER);
        std::fs::create_dir(&selected).unwrap();
        // A PID absent from sysinfo still requires the independent query, whose failure blocks release.
        let owner = Owner {
            locations: Locations::new(vec![selected.clone()]).unwrap(),
            processes: vec![(Pid::from_u32(u32::MAX), 1)],
        };
        assert_eq!(
            wait_released_with::<FailedInspection>(owner, Instant::now()).err().unwrap().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(selected.exists());
        // Native liveness does not need a sysinfo entry (or even a snapshot) to detect this live PID.
        assert!(!Os::exited(std::process::id()).unwrap());
        let reused = Owner {
            locations: Locations::new(vec![selected]).unwrap(),
            processes: vec![(Pid::from_u32(std::process::id()), 1)],
        };
        assert_eq!(
            wait_released_with::<Fixture>(reused, Instant::now()).err().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(Os::exited(0).is_err());
    }

    #[test]
    fn released_cleanup_removes_only_the_allowlist_and_missing_is_success() {
        // TempDir defaults to an ordinary, private directory on all hosts. Canonicalize only the test
        // fixture: production must refuse redirected browser roots, not canonicalize them into trust.
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let selected = root.join(IDENTIFIER);
        std::fs::create_dir(&selected).unwrap();
        std::fs::write(selected.join("cookies"), "old session").unwrap();
        std::fs::write(root.join("other-app"), "keep").unwrap();
        let released = Released(Locations::new(vec![selected.clone()]).unwrap());
        assert!(remove(&released, &root).is_err());
        remove(&released, &selected).unwrap();
        remove(&released, &selected).unwrap();
        assert!(!selected.exists());
        assert_eq!(std::fs::read(root.join("other-app")).unwrap(), b"keep");
    }

    #[test]
    fn native_locations_are_absolute_and_do_not_guess_other_browser_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let paths = locations(temp.path()).unwrap();
        assert!(!paths.paths().is_empty());
        assert!(paths.paths().iter().all(|p| p.is_absolute() && p.ends_with(IDENTIFIER)));
        assert!(Locations::new(vec![PathBuf::from("../browser")]).is_err());
    }

    #[test]
    fn browser_writer_child() {
        let Some(root) = std::env::var_os("SIDEVOICE_WEBVIEW_WRITER_FIXTURE").map(PathBuf::from) else { return };
        std::fs::write(root.join("ready"), b"ready").unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !root.join("exit").exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(20));
        }
        // A browser can flush cookies during shutdown, after its host has requested exit.
        std::fs::write(root.join(IDENTIFIER).join("cookies"), b"last shutdown write").unwrap();
    }

    /// Starts [`browser_writer_child`] in its own process, writing under `directory`.
    pub(super) fn spawn_writer(directory: &Path) -> std::process::Child {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("{}::browser_writer_child", module_path!().split_once("::").unwrap().1)])
            .env("SIDEVOICE_WEBVIEW_WRITER_FIXTURE", directory)
            .spawn()
            .unwrap()
    }

    #[test]
    fn shutdown_writes_finish_before_removal_and_cannot_recreate_storage() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let selected = root.join(IDENTIFIER);
        std::fs::create_dir(&selected).unwrap();
        let mut child = spawn_writer(&root);
        let deadline = Instant::now() + Duration::from_secs(15);
        while !root.join("ready").exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(20));
        }
        let owner = capture_with::<Fixture>(child.id(), Locations::new(vec![selected.clone()]).unwrap()).unwrap();
        std::fs::write(root.join("exit"), b"exit").unwrap();
        // Reap in parallel: on Unix, an unreaped child remains visible as a zombie.
        let reap = thread::spawn(move || child.wait().unwrap());
        let released = wait_released_with::<Fixture>(owner, deadline).unwrap();
        assert_eq!(std::fs::read(selected.join("cookies")).unwrap(), b"last shutdown write");
        remove(&released, &selected).unwrap();
        assert!(reap.join().unwrap().success());
        assert!(!selected.exists());
    }
}
