//! Probe-only logging; native measurements belong to platform/process_memory.

use crate::platform::process_memory::{ProcessMemory, SystemProcessMemory};

/// Print the named measurements in the format CI reads.
#[cfg_attr(test, allow(dead_code))] // Non-macOS unit tests exercise the measurement interface directly.
pub fn report(phase: &str) -> String {
    let line = SystemProcessMemory::sample().line(phase);
    println!("{line}");
    line
}
