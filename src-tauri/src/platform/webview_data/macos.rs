use super::*;

pub struct Os;

impl WebviewData for Os {
    fn preflight(owner: u32) -> io::Result<()> {
        let pid = Pid::from_u32(owner);
        if responsible_pid(pid)? != pid {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "app is not its own responsible process"));
        }
        Ok(())
    }

    fn exited(pid: u32) -> io::Result<bool> {
        let pid = libc::pid_t::try_from(pid).ok().filter(|pid| *pid > 1).ok_or_else(Owner::busy)?;
        // SAFETY: signal zero inspects a positive PID without sending a signal or touching memory.
        if unsafe { libc::kill(pid, 0) } == 0 {
            return Ok(false);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(true)
        } else {
            Err(error)
        }
    }

    fn locations(home: &Path, _app_local_data: &Path) -> io::Result<Locations> {
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
        Owner::responsible_processes(system, processes, |pid, process| {
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
