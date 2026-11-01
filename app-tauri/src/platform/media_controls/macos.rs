//! The macOS body: MediaPlayer's Now Playing and remote commands, and the AirPods mute gesture (`apple.rs`).
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
        // macOS reads the playback state the app sets, and requires it.
        apple::update(app, call, true)
    }
}
