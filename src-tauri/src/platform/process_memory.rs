//! Probe-only process observations. RSS is never relabelled as macOS's physical footprint.

use sidevoice_desktop_core::memory::Sample;

#[cfg(target_os = "macos")]
#[path = "process_memory/macos.rs"]
mod imp;
#[cfg(not(target_os = "macos"))]
mod imp {
    pub struct Os;
    impl super::ProcessMemory for Os {}
}
pub use imp::Os as SystemProcessMemory;

pub trait ProcessMemory {
    /// Sample this process, in bytes. Missing readings stay absent, including the macOS-only footprint,
    /// Metal allocation and vmmap accelerator regions. The portable observation is resident memory.
    fn sample() -> Sample {
        let mut system = sysinfo::System::new();
        let resident = sysinfo::get_current_pid().ok().and_then(|pid| {
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::Some(&[pid]),
                true,
                sysinfo::ProcessRefreshKind::nothing().with_memory(),
            );
            system.process(pid).map(|process| process.memory())
        });
        Sample { resident, ..Sample::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_observations_do_not_invent_footprint_or_gpu_usage() {
        struct Portable;
        impl ProcessMemory for Portable {}
        let sample = Portable::sample();
        assert!(sample.resident.is_some_and(|bytes| bytes > 0));
        assert_eq!(sample.footprint, None);
        assert_eq!(sample.metal, None);
        assert_eq!(sample.accelerator, None);
    }

    #[test]
    fn a_sample_keeps_measurements_separate_and_serializes_missing_values() {
        assert!(SystemProcessMemory::sample().resident.is_some_and(|bytes| bytes > 0));
        assert_eq!(
            Sample::default().line("unknown"),
            "memory phase=unknown footprint=na resident=na metal=na accelerator=na"
        );
    }
}
