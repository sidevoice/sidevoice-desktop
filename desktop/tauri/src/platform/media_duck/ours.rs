//! This app and everything it started: its webview's helper processes make sound and media sessions of their own, and are
//! never "another app's" media. Shared by the OS files that need it (each includes this file).
use sysinfo::{Pid, ProcessesToUpdate, System};

/// A snapshot of the process table, taken once per look.
pub(super) struct Ours(System);

impl Ours {
    pub(super) fn new() -> Self {
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::All, true);
        Ours(system)
    }

    /// Whether `pid` is this process or one that descends from it.
    pub(super) fn has(&self, pid: u32) -> bool {
        let me = Pid::from_u32(std::process::id());
        let mut at = Pid::from_u32(pid);
        for _ in 0..32 {
            if at == me {
                return true;
            }
            match self.0.process(at).and_then(|p| p.parent()) {
                Some(parent) => at = parent,
                None => return false,
            }
        }
        false
    }
}
