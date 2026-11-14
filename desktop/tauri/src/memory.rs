//! Probe-only logging; native measurements belong to platform/diagnostics.

use crate::platform::diagnostics;

/// Print the named measurements in the format CI reads.
#[allow(clippy::disallowed_macros, reason = "the probe's output, read by CI")]
pub fn report(phase: &str) -> String {
    let line = diagnostics::memory().line(phase);
    println!("{line}");
    line
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_report_is_the_line_ci_reads() {
        assert!(super::report("test").starts_with("memory phase=test "));
    }
}
