//! The process's memory as CI measures it (`cargo xtask macos memory`, `macos voice-memory`; #145): what one sample is,
//! the line it is printed as, and how the system's `vmmap` summary is read. Taking the sample is the shell's
//! (src/memory.rs); the pure parts are here so they are tested anywhere. `xtask` reads the lines back by their keys.

/// One look at the process, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sample {
    /// `phys_footprint` of `TASK_VM_INFO`: what Activity Monitor calls Memory, compressed and GPU-wired pages included.
    pub footprint: u64,
    /// The resident set.
    pub resident: u64,
    /// `MTLDevice.currentAllocatedSize` of the system's default device: what the process holds on the GPU through Metal.
    pub metal: Option<u64>,
    /// The dirty and swapped IOAccelerator and IOSurface regions of `vmmap --summary`.
    pub accelerator: Option<u64>,
}

impl Sample {
    /// The line a phase is reported as: `memory phase=NAME footprint=N resident=N metal=N|na accelerator=N|na`.
    pub fn line(&self, phase: &str) -> String {
        let value = |v: Option<u64>| v.map_or("na".to_owned(), |v| v.to_string());
        format!(
            "memory phase={phase} footprint={} resident={} metal={} accelerator={}",
            self.footprint,
            self.resident,
            value(self.metal),
            value(self.accelerator)
        )
    }
}

/// `1234K`, `1.5M`, `2.0G`, `0B` or a bare number (bytes), as `vmmap` writes sizes.
pub fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let (number, unit) = match text.char_indices().find(|(_, c)| c.is_ascii_alphabetic()) {
        Some((at, _)) => text.split_at(at),
        None => (text, ""),
    };
    let scale: f64 = match unit {
        "" | "B" => 1.0,
        "K" => 1024.0,
        "M" => 1024.0 * 1024.0,
        "G" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let number: f64 = number.parse().ok()?;
    (number >= 0.0).then(|| (number * scale).round() as u64)
}

/// What the GPU's regions hold in `vmmap --summary`: the dirty plus swapped size of every IOAccelerator and IOSurface
/// row. A row is its region type's name, then `VIRTUAL RESIDENT DIRTY SWAPPED VOLATILE NONVOLATILE EMPTY` sizes and a
/// count. `None` when the summary has no such row (nothing mapped), which is not an error.
pub fn accelerator_bytes(summary: &str) -> Option<u64> {
    let mut total: Option<u64> = None;
    for row in summary.lines().map(str::trim_start) {
        if !(row.starts_with("IOAccelerator") || row.starts_with("IOSurface")) {
            continue;
        }
        let sizes: Vec<u64> =
            row.split_whitespace().skip_while(|t| parse_size(t).is_none()).map_while(parse_size).collect();
        if let (Some(dirty), Some(swapped)) = (sizes.get(2), sizes.get(3)) {
            *total.get_or_insert(0) += dirty + swapped;
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_read_as_vmmap_writes_them() {
        assert_eq!(parse_size("0K"), Some(0));
        assert_eq!(parse_size("16K"), Some(16 * 1024));
        assert_eq!(parse_size("1.5M"), Some(1_572_864));
        assert_eq!(parse_size("2.0G"), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("4096"), Some(4096));
        assert_eq!(parse_size("(graphics)"), None);
        assert_eq!(parse_size("T"), None);
    }

    #[test]
    fn the_gpus_regions_are_summed_from_their_dirty_and_swapped_sizes() {
        let summary = "\
REGION TYPE                    VIRTUAL   RESIDENT      DIRTY    SWAPPED VOLATILE   NONVOL    EMPTY   REGION COUNT
===========                    =======   ========      =====    ======= ========   ======    =====  ===========
IOAccelerator                    16.0M      8.0M       8.0M       1.0M       0K       0K       0K            2
IOAccelerator (graphics)          2.0M       1M         512K        0K       0K       0K       0K            1
IOSurface                          4M         4M         4M          0K       0K       0K       0K            1
MALLOC_LARGE                     900M       800M       800M        10M       0K       0K       0K           30
";
        assert_eq!(accelerator_bytes(summary), Some(9 * 1024 * 1024 + 512 * 1024 + 4 * 1024 * 1024));
        assert_eq!(accelerator_bytes("MALLOC_LARGE 1M 1M 1M 0K 0K 0K 0K 1"), None);
    }

    #[test]
    fn a_sample_prints_as_the_line_xtask_reads() {
        let sample = Sample { footprint: 10, resident: 20, metal: Some(30), accelerator: None };
        assert_eq!(sample.line("idle"), "memory phase=idle footprint=10 resident=20 metal=30 accelerator=na");
    }
}
