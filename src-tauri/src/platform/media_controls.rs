//! Ownership of headset buttons and media keys, independent of the voice engine.
//!
//! macOS retains Now Playing and the AirPods mute gesture. Other hosts leave
//! ownership with the page's Media Session, including while native voice runs.

use sidevoice_desktop_core::bridge::{CallSnapshot, Command};
use std::sync::Arc;
use tauri::AppHandle;

#[cfg_attr(target_os = "macos", path = "media_controls/macos.rs")]
#[cfg_attr(target_os = "windows", path = "media_controls/windows.rs")]
#[cfg_attr(target_os = "linux", path = "media_controls/linux.rs")]
mod imp;
use imp::Native;

/// Delivers a media action to the call. Implementations preserve main-thread dispatch.
pub type CommandSink = Arc<dyn Fn(Command) + Send + Sync>;

/// What differs per OS: whether the app owns media keys, and its native wiring when it does.
trait Os {
    /// Whether the app owns media keys on this OS.
    const NATIVE: bool = false;

    /// Install process-lifetime callbacks. Unavailable hosts install nothing.
    fn attach(_app: &AppHandle, _sink: CommandSink) {}

    /// Follow call and microphone state. Unavailable hosts leave the page in control.
    fn update(_app: &AppHandle, _call: &CallSnapshot) {}
}

/// `Some("native")` when the app owns media keys; `None` means unavailable,
/// so the page must keep its Media Session handlers and Now Playing element.
pub fn support() -> Option<&'static str> {
    Native::NATIVE.then_some("native")
}

/// Installs process-lifetime callbacks once at startup. Unavailable hosts install nothing.
pub fn attach(app: &AppHandle, sink: CommandSink) {
    Native::attach(app, sink)
}

/// Follows call and microphone state, including navigation and hang-up resets.
/// Native APIs run on the main thread; unavailable hosts leave the page in control.
pub fn update(app: &AppHandle, call: &CallSnapshot) {
    Native::update(app, call)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_receives_the_host_support() {
        assert!(matches!(support(), None | Some("native")));
        let script = sidevoice_desktop_core::bridge::script_for_origin(
            "tauri://localhost",
            false,
            &sidevoice_desktop_core::build_info::BuildInfo::new("dev", "0.0.0", "", ""),
            None,
            &Default::default(),
            false,
            &[],
            support(),
            Default::default(),
        );
        let literal = serde_json::to_string(&support()).unwrap();
        assert!(script.contains(&format!("factory(window, \"tauri://localhost\", {literal},")));
    }
}
