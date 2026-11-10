//! Ownership of headset buttons and media keys, independent of the voice engine.
//!
//! macOS and iOS hold Now Playing and the AirPods mute gesture (`apple.rs`); Android a media session (Kotlin,
//! `android/…/MediaControlsPlugin.kt`), which the call's notification shows. Windows and Linux leave ownership with
//! the page's Media Session, including while native voice runs.

use sidevoice_shared_headset::HeadsetCapabilities;
use std::sync::Arc;
use tauri::AppHandle;

#[cfg_attr(target_os = "macos", path = "media_controls/macos.rs")]
#[cfg_attr(target_os = "ios", path = "media_controls/ios.rs")]
#[cfg_attr(target_os = "android", path = "media_controls/android.rs")]
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
    /// The double press has an action the person chose: Android's media session advertises skip-to-next only then (macOS
    /// always registers the next-track command).
    pub double_press_wanted: bool,
    /// The single press the person chose leaves the microphone alone (say one thing, skip, nothing): the AirPods gesture then
    /// runs it and the system's mute is refused, so the system never says "muted" while the call listens. False (the mute, the
    /// default) where the app does not say.
    pub press_keeps_mic: bool,
}

/// What a media action asks of the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCommand {
    ToggleMute,
    /// `nextTrackCommand`, on Apple systems, and the media session's skip-to-next on Android: what a headset's or AirPods' double
    /// press of play/pause arrives as.
    NextTrack,
}

/// Delivers a media action to the call. Implementations preserve main-thread dispatch.
pub type CommandSink = Arc<dyn Fn(MediaCommand) + Send + Sync>;

/// What differs per OS: whether the app owns media keys, and its native wiring when it does.
trait Os {
    /// Whether the app owns media keys on this OS.
    const NATIVE: bool = false;

    /// What a headset's presses can reach the app with during a call: both presses, each with any action, unless the OS
    /// delivers less.
    const PRESSES: HeadsetCapabilities = HeadsetCapabilities::EVERY;

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

/// What a headset's presses can do here during a call (`HeadsetStatus.actions`, docs/BRIDGE.md → "The shortcuts"): the app's
/// choices for a press are only these, and a press that does not reach the app has no row.
pub fn presses() -> HeadsetCapabilities {
    Native::PRESSES
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
