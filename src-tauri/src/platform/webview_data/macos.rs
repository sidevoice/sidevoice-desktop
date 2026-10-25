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
        unix_pid_exited(pid)
    }

    fn locations(home: &Path, _app_local_data: &Path) -> io::Result<Locations> {
        locations(vec![
            home.join("Library/Application Support").join(IDENTIFIER),
            home.join("Library/WebKit").join(IDENTIFIER),
            home.join("Library/Caches").join(IDENTIFIER),
        ])
    }

    fn observe(system: &System, _locations: &Locations, processes: &mut Vec<(Pid, u64)>) -> io::Result<()> {
        let &(app, start) = processes.first().ok_or_else(busy)?;
        if system.process(app).is_some_and(|p| p.start_time() == start) && responsible_pid(app)? != app {
            // A terminal-launched app can inherit the terminal's responsibility. Do not confuse its
            // WebKit services with other applications charged to that same responsible process.
            return Err(busy());
        }
        let current = system.process(Pid::from_u32(std::process::id())).ok_or_else(busy)?;
        let user = current.user_id().ok_or_else(busy)?;
        let mut tree = processes.clone();
        descendants(system, &mut tree);
        responsible_processes(system, processes, |pid, process| {
            if !process.name().to_string_lossy().contains("WebKit") {
                return Ok(None);
            }
            if process.user_id().ok_or_else(busy)? != user {
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
        return Err(busy());
    }
    // SAFETY: the non-null symbol has this system ABI; no library handle is closed while in use.
    let responsible: unsafe extern "C" fn(libc::pid_t) -> libc::pid_t = unsafe { std::mem::transmute(symbol) };
    let pid = libc::pid_t::try_from(pid.as_u32()).map_err(|_| busy())?;
    // SAFETY: this query takes a PID by value and does not access caller-owned memory.
    let owner = unsafe { responsible(pid) };
    if owner <= 0 {
        Err(busy())
    } else {
        Ok(Pid::from_u32(owner as u32))
    }
}
