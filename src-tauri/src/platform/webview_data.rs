//! Browser storage owned by the desktop reset. The transaction owns admission and the launch barrier.

use sidevoice_platform::fs::{Access, Directory, HeldDirectory};
use sidevoice_platform::removal::Removal;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::{io, thread};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[cfg(target_os = "macos")]
#[path = "webview_data/macos.rs"]
mod imp;
#[cfg(target_os = "linux")]
mod imp {
    pub struct Os;
    impl super::WebviewData for Os {}
}
#[cfg(target_os = "windows")]
#[path = "webview_data/windows.rs"]
mod imp;
pub use imp::Os as SystemWebviewData;

#[cfg(any(target_os = "macos", test))]
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

/// Browser cleanup only: no service stop, reset lock, journal, relaunch or app removal policy.
pub trait WebviewData {
    /// Resolve the runtime's roots from native Tauri `home_dir` and `app_local_data_dir` results.
    /// Call before exit; carry these native values through authenticated helper handoff. macOS keeps
    /// its existing WKWebView default store. Linux's Wry context uses the same base for data AND cache.
    fn locations(_home: &Path, app_local_data: &Path) -> io::Result<Locations> {
        // Tauri supplies this path to Wry. WebView2 uses it as its UDF; WebKitGTK uses it
        // for BOTH base_data_directory and base_cache_directory, with cookies at <path>/cookies.
        locations(vec![app_local_data.to_path_buf()])
    }

    /// Snapshot the app and its browser descendants before allowing the app to exit. The transaction
    /// must already forbid new webviews and keep other app launches behind its held reset lock.
    /// Windows also discovers every runtime using this user-data folder, including reparented ones.
    fn capture(owner: u32, locations: Locations) -> io::Result<Owner> {
        let system = snapshot()?;
        let pid = Pid::from_u32(owner);
        let app = system.process(pid).ok_or_else(busy)?;
        if app.start_time() == 0 {
            return Err(busy());
        }
        let mut processes = vec![(pid, app.start_time())];
        Self::observe(&system, &locations, &mut processes)?;
        Ok(Owner { locations, processes })
    }

    /// Wait for the app and its observed browser processes, refreshing descendants and folder users
    /// until `deadline`. Never kill unrelated browsers or treat a timeout as permission to delete.
    /// The app must stop creating webviews before capture; the helper must retain the launch barrier
    /// through removal. A fresh process must never initialize writers between release and deletion.
    fn wait_released(mut owner: Owner, deadline: Instant) -> io::Result<Released> {
        loop {
            let system = snapshot()?;
            Self::observe(&system, &owner.locations, &mut owner.processes)?;
            if owner.processes.iter().any(|(pid, _)| system.process(*pid).is_some_and(|p| p.start_time() == 0)) {
                return Err(busy());
            }
            if owner
                .processes
                .iter()
                .all(|(pid, start)| system.process(*pid).is_none_or(|process| process.start_time() != *start))
            {
                return Ok(Released(owner.locations));
            }
            if Instant::now() >= deadline {
                return Err(busy());
            }
            thread::sleep(Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())));
        }
    }

    /// Add browser writers to the pre-exit observation. `sysinfo` owns native process enumeration;
    /// selection is desktop policy. Kept on this trait so OS implementations expose no extra API.
    fn observe(system: &System, _locations: &Locations, processes: &mut Vec<(Pid, u64)>) -> io::Result<()> {
        descendants(system, processes);
        Ok(())
    }

    /// Remove one exact allowlisted root only after browser release, using core's held-parent removal.
    /// No directory is created or repaired; a missing root is success. Redirected roots are refused.
    /// The caller keeps recovery records outside these roots and retains its reset/launch lock.
    fn remove(released: &Released, path: &Path) -> io::Result<()> {
        if !released.0.paths().iter().any(|allowed| allowed == path) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "outside webview roots"));
        }
        // Opening the target first rejects root symlinks/junctions rather than unlinking them.
        let target = match HeldDirectory::open(path, Access::NotWritableByOthers) {
            Ok(target) => target,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let parent = path.parent().ok_or_else(busy)?;
        let name = path.file_name().ok_or_else(busy)?;
        let held = HeldDirectory::open(parent, Access::NotWritableByOthers)?;
        held.check_contents(&[name])?;
        // Windows denies deleting the target while the target's own held handle is open. The parent
        // remains held, and Removal repeats identity/ownership/mount checks before and during deletion.
        drop(target);
        held.remove_contents(&[name])
    }
}

fn busy() -> io::Error {
    io::Error::new(io::ErrorKind::WouldBlock, "webview writers have not been released")
}

fn locations(paths: Vec<PathBuf>) -> io::Result<Locations> {
    if paths.iter().any(|path| {
        !path.is_absolute()
            || path.parent().is_none()
            || path.components().any(|part| matches!(part, std::path::Component::ParentDir))
    }) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid native webview root"));
    }
    Ok(Locations(paths))
}

fn snapshot() -> io::Result<System> {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always).with_user(UpdateKind::Always),
    );
    // Enumeration failure must never be mistaken for an empty machine.
    system.process(Pid::from_u32(std::process::id())).ok_or_else(busy)?;
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

// The browser's command line is used only to associate a process with our UDF, never as a deletion path.
#[cfg(any(target_os = "windows", test))]
fn user_data_folder(args: &[std::ffi::OsString]) -> io::Result<Option<PathBuf>> {
    for (index, arg) in args.iter().enumerate() {
        if arg == "--user-data-dir" {
            return args.get(index + 1).map(PathBuf::from).map(Some).ok_or_else(busy);
        }
        let text = arg.to_str().ok_or_else(busy)?;
        if let Some(folder) = text.strip_prefix("--user-data-dir=") {
            return if folder.is_empty() { Err(busy()) } else { Ok(Some(PathBuf::from(folder))) };
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A native test subprocess stands in for a browser, so exercise the shared lifetime contract
    // without pretending that its executable/arguments identify a real WebView2 runtime.
    struct Fixture;
    impl WebviewData for Fixture {}

    #[test]
    fn live_owner_prevents_removal_even_without_a_browser() {
        let temp = tempfile::tempdir().unwrap();
        let paths = locations(vec![temp.path().join(IDENTIFIER)]).unwrap();
        let owner = Fixture::capture(std::process::id(), paths).unwrap();
        assert_eq!(Fixture::wait_released(owner, Instant::now()).err().unwrap().kind(), io::ErrorKind::WouldBlock);
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
        let released = Released(locations(vec![selected.clone()]).unwrap());
        assert!(SystemWebviewData::remove(&released, &root).is_err());
        SystemWebviewData::remove(&released, &selected).unwrap();
        SystemWebviewData::remove(&released, &selected).unwrap();
        assert!(!selected.exists());
        assert_eq!(std::fs::read(root.join("other-app")).unwrap(), b"keep");
    }

    #[test]
    fn native_locations_are_absolute_and_do_not_guess_other_browser_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join(IDENTIFIER);
        let paths = SystemWebviewData::locations(temp.path(), &local).unwrap();
        assert!(!paths.paths().is_empty());
        assert!(paths.paths().iter().all(|p| p.is_absolute() && p.ends_with(IDENTIFIER)));
        assert!(locations(vec![PathBuf::from("../browser")]).is_err());
    }

    #[test]
    fn user_data_folder_is_an_exact_argument_not_a_command_substring() {
        let args = |items: &[&str]| items.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        for command in [
            args(&["browser", "--user-data-dir=C:\\Users\\Name With Spaces\\é"]),
            args(&["browser", "--user-data-dir", "C:\\Users\\Name With Spaces\\é"]),
        ] {
            assert_eq!(user_data_folder(&command).unwrap(), Some(PathBuf::from("C:\\Users\\Name With Spaces\\é")));
        }
        assert_eq!(user_data_folder(&args(&["browser", "--something=--user-data-dir=other"])).unwrap(), None);
        assert!(user_data_folder(&args(&["browser", "--user-data-dir"])).is_err());
        assert!(user_data_folder(&args(&["browser", "--user-data-dir="])).is_err());
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

    #[test]
    fn shutdown_writes_finish_before_removal_and_cannot_recreate_storage() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let selected = root.join(IDENTIFIER);
        std::fs::create_dir(&selected).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("{}::browser_writer_child", module_path!().split_once("::").unwrap().1),
                "--nocapture",
            ])
            .env("SIDEVOICE_WEBVIEW_WRITER_FIXTURE", &root)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !root.join("ready").exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(20));
        }
        let owner = Fixture::capture(child.id(), locations(vec![selected.clone()]).unwrap()).unwrap();
        std::fs::write(root.join("exit"), b"exit").unwrap();
        // Reap in parallel: on Unix, an unreaped child remains visible as a zombie.
        let reap = thread::spawn(move || child.wait().unwrap());
        let released = Fixture::wait_released(owner, deadline).unwrap();
        assert_eq!(std::fs::read(selected.join("cookies")).unwrap(), b"last shutdown write");
        SystemWebviewData::remove(&released, &selected).unwrap();
        assert!(reap.join().unwrap().success());
        assert!(!selected.exists());
    }
}
