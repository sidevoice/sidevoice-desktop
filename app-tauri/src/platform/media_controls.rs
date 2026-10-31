//! Ownership of headset buttons and media keys, independent of the voice engine.
//!
//! macOS retains Now Playing and the AirPods mute gesture. Other hosts leave
//! ownership with the page's Media Session, including while native voice runs.

use std::sync::Arc;
use tauri::AppHandle;

#[cfg_attr(target_os = "macos", path = "media_controls/macos.rs")]
#[cfg_attr(target_os = "windows", path = "media_controls/windows.rs")]
#[cfg_attr(target_os = "linux", path = "media_controls/linux.rs")]
mod imp;
use imp::Native;

/// What the headset buttons and Now Playing need to know of the call, as the app reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallState {
    /// The room's controller is up.
    pub ready: bool,
    /// In a call, or joining one.
    pub joined: bool,
    pub mic_enabled: bool,
    /// The conversation being looked at: Now Playing's title.
    pub title: String,
}

/// What a media action asks of the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCommand {
    ToggleMute,
}

/// Delivers a media action to the call. Implementations preserve main-thread dispatch.
pub type CommandSink = Arc<dyn Fn(MediaCommand) + Send + Sync>;

/// What differs per OS: whether the app owns media keys, and its native wiring when it does.
trait Os {
    /// Whether the app owns media keys on this OS.
    const NATIVE: bool = false;

    /// Install process-lifetime callbacks. Unavailable hosts install nothing.
    fn attach(_app: &AppHandle, _sink: CommandSink) {}

    /// Follow call and microphone state. Unavailable hosts leave the page in control.
    fn update(_app: &AppHandle, _call: &CallState) {}
}

/// Whether the app owns the system's media controls here. Where it does not, the page keeps its Media Session handlers
/// and its Now Playing element.
pub fn available() -> bool {
    Native::NATIVE
}

/// Installs process-lifetime callbacks once at startup. Unavailable hosts install nothing.
pub fn attach(app: &AppHandle, sink: CommandSink) {
    Native::attach(app, sink)
}

/// Follows call and microphone state, including navigation and hang-up resets.
/// Native APIs run on the main thread; unavailable hosts leave the page in control.
pub fn update(app: &AppHandle, call: &CallState) {
    Native::update(app, call)
}
