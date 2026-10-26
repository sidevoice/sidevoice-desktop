//! What CI's exit run observes of the process ending (src/probe.rs). Probe builds only.

pub trait ExitDiagnostics {
    /// Registers, before any model is loaded, a handler that logs `exit handlers ran` when `exit()` reaches it. It runs
    /// after the destructors of the native libraries linked in, so its line says they ran to their end. Hosts whose
    /// exit has no such destructors to observe register nothing.
    fn report_handlers_ran() {}
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::NativeExitDiagnostics;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::NativeExitDiagnostics;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::NativeExitDiagnostics;

#[cfg(test)]
mod tests {
    use super::*;

    struct Unobserved;
    impl ExitDiagnostics for Unobserved {}

    #[test]
    fn a_host_with_nothing_to_observe_registers_nothing() {
        Unobserved::report_handlers_ran();
    }
}
