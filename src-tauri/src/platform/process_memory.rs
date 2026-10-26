//! Probe-only process observations. RSS is never relabelled as macOS's physical footprint.

use sidevoice_desktop_core::memory::Sample;

#[cfg_attr(target_os = "macos", path = "process_memory/macos.rs")]
#[cfg_attr(target_os = "windows", path = "process_memory/windows.rs")]
#[cfg_attr(target_os = "linux", path = "process_memory/linux.rs")]
mod imp;
use imp::Native;

/// The readings only some OSes have, in bytes; `None` where this OS has no such reading.
trait Os {
    /// The physical footprint (macOS `task_info`).
    fn footprint() -> Option<u64> {
        None
    }

    /// What the process holds on the GPU (macOS Metal).
    fn metal() -> Option<u64> {
        None
    }

    /// The GPU's mapped regions (macOS `vmmap`).
    fn accelerator() -> Option<u64> {
        None
    }
}

/// Sample this process, in bytes. Missing readings stay absent, including the macOS-only footprint,
/// Metal allocation and vmmap accelerator regions. The portable observation is resident memory.
pub fn sample() -> Sample {
    Sample {
        footprint: Native::footprint(),
        resident: resident(),
        metal: Native::metal(),
        accelerator: Native::accelerator(),
    }
}

/// Resident memory, through the maintained portable API.
fn resident() -> Option<u64> {
    let mut system = sysinfo::System::new();
    let pid = sysinfo::get_current_pid().ok()?;
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
        sysinfo::ProcessRefreshKind::nothing().with_memory(),
    );
    system.process(pid).and_then(|process| resident_bytes(process.memory()))
}

// A live process cannot have an empty resident set. In particular, sysinfo leaves its initial zero on
// Windows when GetProcessMemoryInfo fails. Preserve that as unavailable rather than a measured zero.
fn resident_bytes(bytes: u64) -> Option<u64> {
    (bytes > 0).then_some(bytes)
}

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
        impl Os for Portable {}
        assert_eq!(Portable::footprint(), None);
        assert_eq!(Portable::metal(), None);
        assert_eq!(Portable::accelerator(), None);
    }

    #[test]
    fn a_sample_keeps_measurements_separate_and_serializes_missing_values() {
        assert!(sample().resident.is_some_and(|bytes| bytes > 0));
        assert_eq!(
            Sample::default().line("unknown"),
            "memory phase=unknown footprint=na resident=na metal=na accelerator=na"
        );
    }
}
