//! What the app's own voice call holds, for CI's memory run (src/probe.rs). Probe builds only.
//!
//! The call runs in the app on macOS only; elsewhere the page runs it, so there is nothing to report.

use tauri::AppHandle;

pub trait VoiceHolds {
    /// What the app's voice host holds now (`keeps=N lanes=M`), or `None` when this host runs no call of its own.
    fn held(_app: &AppHandle) -> Option<String> {
        None
    }
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::NativeVoiceHolds;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::NativeVoiceHolds;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::NativeVoiceHolds;

#[cfg(test)]
mod tests {
    use super::*;

    struct Unavailable;
    impl VoiceHolds for Unavailable {}

    #[test]
    fn a_host_without_a_call_of_its_own_holds_nothing_to_report() {
        fn assert_holds<T: VoiceHolds>() {}
        assert_holds::<NativeVoiceHolds>();
        assert_holds::<Unavailable>();
    }
}
