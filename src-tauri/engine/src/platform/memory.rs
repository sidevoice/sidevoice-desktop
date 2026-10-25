//! Physical memory observations, without model-selection policy.

#[cfg(target_os = "macos")]
#[path = "memory/macos.rs"]
mod imp;
#[cfg(not(target_os = "macos"))]
mod imp {
    pub struct Os;
    impl super::Memory for Os {}
}
pub use imp::Os as SystemMemory;

pub struct MemorySnapshot {
    pub total_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
}

impl MemorySnapshot {
    pub fn total_mb(&self) -> Option<u64> {
        self.total_bytes.map(|bytes| bytes / (1024 * 1024)).filter(|mb| *mb > 0)
    }

    pub fn available_mb(&self) -> Option<u64> {
        self.available_bytes.map(|bytes| bytes / (1024 * 1024))
    }
}

pub trait Memory {
    /// Total physical RAM and RAM available now, in bytes. Windows/Linux use sysinfo's OS observations;
    /// macOS overrides with the existing kernel pressure-level estimate (not a hard allocation limit).
    /// Unknown readings stay absent; a valid zero available reading stays zero.
    fn snapshot() -> MemorySnapshot {
        let mut system = sysinfo::System::new();
        system.refresh_memory();
        let total = system.total_memory();
        MemorySnapshot {
            total_bytes: (total > 0).then_some(total),
            available_bytes: (total > 0).then(|| system.available_memory()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_machine_reports_physical_memory_and_availability() {
        let snapshot = SystemMemory::snapshot();
        let total = snapshot.total_mb().expect("the OS reports physical memory");
        assert!(total >= 256, "{total} MiB");
        assert!(snapshot.available_mb().expect("available memory") <= total);
    }

    #[test]
    fn absent_and_zero_observations_keep_their_meaning() {
        let absent = MemorySnapshot { total_bytes: None, available_bytes: None };
        assert_eq!(absent.total_mb(), None);
        assert_eq!(absent.available_mb(), None);
        let empty = MemorySnapshot { total_bytes: Some(0), available_bytes: Some(0) };
        assert_eq!(empty.total_mb(), None);
        assert_eq!(empty.available_mb(), Some(0));
    }
}
