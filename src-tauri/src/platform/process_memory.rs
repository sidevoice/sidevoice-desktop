//! Probe-only process observations. RSS is never relabelled as macOS's physical footprint.

use sidevoice_desktop_core::memory::Sample;

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
            system.process(pid).and_then(|process| resident_bytes(process.memory()))
        });
        Sample { resident, ..Sample::default() }
    }
}

// A live process cannot have an empty resident set. In particular, sysinfo leaves its initial zero on
// Windows when GetProcessMemoryInfo fails. Preserve that as unavailable rather than a measured zero.
fn resident_bytes(bytes: u64) -> Option<u64> {
    (bytes > 0).then_some(bytes)
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::SystemProcessMemory;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::SystemProcessMemory;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::SystemProcessMemory;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_live_process_reading_is_unavailable_in_the_probe_line() {
        let sample = Sample { resident: resident_bytes(0), ..Sample::default() };
        assert_eq!(sample.resident, None);
        assert_eq!(sample.line("failed"), "memory phase=failed footprint=na resident=na metal=na accelerator=na");
        assert_eq!(resident_bytes(4096), Some(4096));
    }

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
