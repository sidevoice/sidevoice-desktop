//! The iOS body: the same MediaPlayer code as macOS (`apple.rs`): the call on the lock screen and in Control Center, its
//! play/pause from there and from a headset, and the AirPods mute gesture (iOS 17+).
use super::{CallState, CommandSink, Os};
use tauri::AppHandle;

#[path = "apple.rs"]
mod apple;

pub(super) struct Native;

impl Os for Native {
    const NATIVE: bool = true;

    fn attach(app: &AppHandle, sink: CommandSink) {
        apple::attach(app, sink)
    }

    fn update(app: &AppHandle, call: &CallState) {
        // iOS takes the playback state from the app's audio session.
        apple::update(app, call, false)
    }
}
