//! What CI's exit run observes of the process ending (src/probe.rs). Probe builds only.

#[cfg_attr(target_os = "macos", path = "exit_diagnostics/macos.rs")]
#[cfg_attr(target_os = "windows", path = "exit_diagnostics/windows.rs")]
#[cfg_attr(target_os = "linux", path = "exit_diagnostics/linux.rs")]
mod imp;
use imp::Os;

/// What differs per OS: whether its exit runs native destructors worth observing.
trait Native {
    /// Register the exit handler. Hosts whose exit has no such destructors to observe register nothing.
    fn report_handlers_ran() {}
}

/// Registers, before any model is loaded, a handler that logs `exit handlers ran` when `exit()` reaches it. It runs
/// after the destructors of the native libraries linked in, so its line says they ran to their end. Hosts whose
/// exit has no such destructors to observe register nothing.
pub fn report_handlers_ran() {
    Os::report_handlers_ran()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Unobserved;
    impl Native for Unobserved {}

    #[test]
    fn a_host_with_nothing_to_observe_registers_nothing() {
        Unobserved::report_handlers_ran();
    }
}
