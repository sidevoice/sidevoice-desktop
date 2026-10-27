use super::*;
use nix::errno::Errno;
use nix::sys::signal::kill;

pub(super) struct Native;

impl Os for Native {
    fn preflight(owner: u32) -> io::Result<()> {
        let pid = Pid::from_u32(owner);
        if responsible_pid(pid)? != pid {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "app is not its own responsible process"));
        }
        Ok(())
    }

    fn exited(pid: u32) -> io::Result<bool> {
        let pid = libc::pid_t::try_from(pid).ok().filter(|pid| *pid > 1).ok_or_else(Owner::busy)?;
        // Signal zero (`None`) inspects a positive PID without sending a signal.
        match kill(nix::unistd::Pid::from_raw(pid), None) {
            Ok(()) => Ok(false),
            Err(Errno::ESRCH) => Ok(true),
            Err(error) => Err(error.into()),
        }
    }

    fn locations(home: &Path) -> io::Result<Locations> {
        Locations::new(vec![
            home.join("Library/Application Support").join(IDENTIFIER),
            home.join("Library/WebKit").join(IDENTIFIER),
            home.join("Library/Caches").join(IDENTIFIER),
        ])
    }

    fn observe(system: &System, _locations: &Locations, processes: &mut Vec<(Pid, u64)>) -> io::Result<()> {
        let &(app, start) = processes.first().ok_or_else(Owner::busy)?;
        if system.process(app).is_some_and(|p| p.start_time() == start) && responsible_pid(app)? != app {
            // A terminal-launched app can inherit the terminal's responsibility. Do not confuse its
            // WebKit services with other applications charged to that same responsible process.
            return Err(Owner::busy());
        }
        let current = system.process(Pid::from_u32(std::process::id())).ok_or_else(Owner::busy)?;
        let user = current.user_id().ok_or_else(Owner::busy)?;
        let mut tree = processes.clone();
        Owner::descendants(system, &mut tree);
        responsible_processes(system, processes, |pid, process| {
            if !process.name().to_string_lossy().contains("WebKit") {
                return Ok(None);
            }
            if process.user_id().ok_or_else(Owner::busy)? != user {
                return Ok(None);
            }
            if tree.contains(&(pid, process.start_time())) {
                return Ok(Some(app));
            }
            responsible_pid(pid).map(Some)
        })
    }
}

// Responsibility, unlike PPID, associates launchd-owned XPC services with their application. Keep
// already observed PID/start pairs even after the app exits or macOS changes the responsibility chain.
fn responsible_processes(
    system: &System,
    processes: &mut Vec<(Pid, u64)>,
    mut lookup: impl FnMut(Pid, &sysinfo::Process) -> io::Result<Option<Pid>>,
) -> io::Result<()> {
    let app = processes.first().ok_or_else(Owner::busy)?.0;
    for (pid, process) in system.processes() {
        if processes.contains(&(*pid, process.start_time())) {
            continue;
        }
        if lookup(*pid, process)? == Some(app) {
            if process.start_time() == 0 {
                return Err(Owner::busy());
            }
            let identity = (*pid, process.start_time());
            if !processes.contains(&identity) {
                processes.push(identity);
            }
        }
    }
    Ok(())
}

fn responsible_pid(pid: Pid) -> io::Result<Pid> {
    // sysinfo has no responsibility API; resolve the macOS SPI at runtime and fail closed if
    // unavailable. Its pid_t -> pid_t ABI is also used by Chromium's process_info_mac implementation.
    // SAFETY: dlsym reads a static NUL-terminated symbol in the process's loaded system libraries.
    let symbol = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"responsibility_get_pid_responsible_for_pid".as_ptr()) };
    if symbol.is_null() {
        return Err(Owner::busy());
    }
    // SAFETY: the non-null symbol has this system ABI; no library handle is closed while in use.
    let responsible: unsafe extern "C" fn(libc::pid_t) -> libc::pid_t = unsafe { std::mem::transmute(symbol) };
    let pid = libc::pid_t::try_from(pid.as_u32()).map_err(|_| Owner::busy())?;
    // SAFETY: this query takes a PID by value and does not access caller-owned memory.
    let owner = unsafe { responsible(pid) };
    if owner <= 0 {
        Err(Owner::busy())
    } else {
        Ok(Pid::from_u32(owner as u32))
    }
}

// Responsibility association is macOS-only (no other host has launchd-owned browser services), so this
// test cannot be phrased against the shared interface; the shared lifetime tests cover the rest.
#[cfg(test)]
mod tests {
    use super::super::tests::{remove_at, spawn_writer, Fixture};
    use super::*;

    #[test]
    fn responsible_sibling_writer_blocks_removal_after_host_exit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let selected = root.join(IDENTIFIER);
        std::fs::create_dir(&selected).unwrap();
        let host_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(host_dir.path().join(IDENTIFIER)).unwrap();
        let mut host = spawn_writer(host_dir.path());
        let mut writer = spawn_writer(&root);
        let deadline = Instant::now() + Duration::from_secs(15);
        while !root.join("ready").exists() || !host_dir.path().join("ready").exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(20));
        }
        let mut owner = capture_with::<Fixture>(host.id(), Locations::new(vec![selected.clone()]).unwrap()).unwrap();
        let writer_pid = Pid::from_u32(writer.id());
        assert!(!owner.processes.iter().any(|(pid, _)| *pid == writer_pid));
        let system = Owner::snapshot().unwrap();
        // Lookup failure cannot authorize cleanup; another responsible app must not be selected.
        assert!(responsible_processes(&system, &mut owner.processes, |_, _| Err(Owner::busy())).is_err());
        responsible_processes(&system, &mut owner.processes, |_, _| Ok(Some(Pid::from_u32(std::process::id()))))
            .unwrap();
        assert!(!owner.processes.iter().any(|(pid, _)| *pid == writer_pid));
        // Inject only the platform lookup: these are real sibling processes, not descendants.
        responsible_processes(&system, &mut owner.processes, |pid, _| {
            Ok((pid == writer_pid).then_some(Pid::from_u32(host.id())))
        })
        .unwrap();
        let retry =
            Owner { locations: Locations::new(vec![selected.clone()]).unwrap(), processes: owner.processes.clone() };
        std::fs::write(host_dir.path().join("exit"), b"exit").unwrap();
        assert!(host.wait().unwrap().success());
        assert_eq!(
            wait_released_with::<Fixture>(owner, Instant::now()).err().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(selected.exists());
        std::fs::write(root.join("exit"), b"exit").unwrap();
        assert!(writer.wait().unwrap().success());
        let released = wait_released_with::<Fixture>(retry, deadline).unwrap();
        assert_eq!(std::fs::read(selected.join("cookies")).unwrap(), b"last shutdown write");
        remove_at(&released, &selected).unwrap();
        assert!(!selected.exists());
    }
}
