//! What the app's own voice call holds, for CI's memory run (src/probe.rs). Probe builds only.
//!
//! The call runs in the app on macOS only; elsewhere the page runs it, so there is nothing to report. That
//! difference is `crate::voice` being macOS-only; this module goes once the app's voice call is host-agnostic.

use tauri::{AppHandle, Runtime};

#[cfg_attr(target_os = "macos", path = "voice_holds/macos.rs")]
#[cfg_attr(target_os = "windows", path = "voice_holds/windows.rs")]
#[cfg_attr(target_os = "linux", path = "voice_holds/linux.rs")]
mod imp;
use imp::Os;

/// What differs per OS: whether the app runs a call of its own.
trait Native {
    /// What the app's voice host holds now; `None` where the app runs no call of its own.
    fn held<R: Runtime>(_app: &AppHandle<R>) -> Option<String> {
        None
    }
}

/// What the app's voice host holds now (`keeps=N lanes=M`), or `None` when this host runs no call of its own.
pub fn held<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    Os::held(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_without_a_call_of_its_own_holds_nothing_to_report() {
        let app = tauri::test::mock_builder().build(tauri::test::mock_context(tauri::test::noop_assets())).unwrap();
        assert_eq!(held(app.handle()), None);
    }
}
