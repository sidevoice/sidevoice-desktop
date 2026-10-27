use super::*;
use nix::errno::Errno;
use nix::sys::signal::kill;

pub(super) struct Native;

impl Os for Native {
    fn exited(pid: u32) -> io::Result<bool> {
        let pid = libc::pid_t::try_from(pid).ok().filter(|pid| *pid > 1).ok_or_else(Owner::busy)?;
        // Signal zero (`None`) inspects a positive PID without sending a signal.
        match kill(nix::unistd::Pid::from_raw(pid), None) {
            Ok(()) => Ok(false),
            Err(Errno::ESRCH) => Ok(true),
            Err(error) => Err(error.into()),
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
