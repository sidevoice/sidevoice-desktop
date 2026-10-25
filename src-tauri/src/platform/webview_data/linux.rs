use super::*;

pub struct Os;

impl WebviewData for Os {
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

    fn observe(system: &System, _locations: &Locations, processes: &mut Vec<(Pid, u64)>) -> io::Result<()> {
        let mut tree = processes.clone();
        Owner::descendants(system, &mut tree);
        for (pid, start) in tree {
            if !processes.contains(&(pid, start))
                && system.process(pid).is_some_and(|p| p.name().to_string_lossy().contains("WebKit"))
            {
                processes.push((pid, start));
            }
        }
        Ok(())
    }
}
