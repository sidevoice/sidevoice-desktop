//! Ownership of headset buttons and media keys, independent of the voice engine.
//!
//! macOS retains Now Playing and the AirPods mute gesture. Other hosts leave
//! ownership with the page's Media Session, including while native voice runs.

use sidevoice_desktop_core::bridge::{CallSnapshot, Command};
use std::sync::Arc;
use tauri::AppHandle;

/// Delivers a media action to the call. Implementations preserve main-thread dispatch.
pub type CommandSink = Arc<dyn Fn(Command) + Send + Sync>;

pub trait MediaControls {
    /// `Some("native")` when the app owns media keys; `None` means unavailable,
    /// so the page must keep its Media Session handlers and Now Playing element.
    fn support() -> Option<&'static str> {
        None
    }

    /// Installs process-lifetime callbacks once at startup. Unavailable hosts install nothing.
    fn attach(_app: &AppHandle, _sink: CommandSink) {}

    /// Follows call and microphone state, including navigation and hang-up resets.
    /// Native APIs run on the main thread; unavailable hosts leave the page in control.
    fn update(_app: &AppHandle, _call: &CallSnapshot) {}
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::NativeMediaControls;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::NativeMediaControls;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::NativeMediaControls;

#[cfg(test)]
mod tests {
    use super::*;

    struct Unavailable;
    impl MediaControls for Unavailable {}

    #[test]
    fn unavailable_leaves_media_keys_to_the_page() {
        assert_eq!(Unavailable::support(), None);
        assert!(matches!(NativeMediaControls::support(), None | Some("native")));
    }

    #[test]
    fn bridge_receives_the_implementation_support() {
        fn assert_bridge_support<T: MediaControls>() {
            let support = T::support();
            let script = sidevoice_desktop_core::bridge::script_for_origin(
                "tauri://localhost",
                false,
                &sidevoice_desktop_core::build_info::BuildInfo::new("dev", "0.0.0", "", ""),
                None,
                &Default::default(),
                false,
                &[],
                support,
            );
            let literal = serde_json::to_string(&support).unwrap();
            assert!(script.contains(&format!("factory(window, \"tauri://localhost\", {literal},")));
        }

        assert_bridge_support::<Unavailable>();
        assert_bridge_support::<NativeMediaControls>();
    }
}
