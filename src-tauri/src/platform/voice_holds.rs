//! What the app's own voice call holds, for CI's memory run (src/probe.rs). Probe builds only.
//!
//! The call runs in the app on macOS only; elsewhere the page runs it, so there is nothing to report.

use tauri::{AppHandle, Runtime};

pub trait VoiceHolds {
    /// What the app's voice host holds now (`keeps=N lanes=M`), or `None` when this host runs no call of its own.
    fn held<R: Runtime>(_app: &AppHandle<R>) -> Option<String> {
        None
    }
}

#[cfg(all(feature = "probe", target_os = "macos"))]
mod macos;
#[cfg(all(feature = "probe", target_os = "macos"))]
pub use macos::NativeVoiceHolds;

#[cfg(all(feature = "probe", target_os = "windows"))]
mod windows;
#[cfg(all(feature = "probe", target_os = "windows"))]
pub use windows::NativeVoiceHolds;

#[cfg(all(feature = "probe", target_os = "linux"))]
mod linux;
#[cfg(all(feature = "probe", target_os = "linux"))]
pub use linux::NativeVoiceHolds;

#[cfg(test)]
mod tests {
    use super::*;

    struct Unavailable;
    impl VoiceHolds for Unavailable {}

    #[test]
    fn a_host_without_a_call_of_its_own_holds_nothing_to_report() {
        let app = tauri::test::mock_builder().build(tauri::test::mock_context(tauri::test::noop_assets())).unwrap();
        assert_eq!(Unavailable::held(app.handle()), None);
    }
}
