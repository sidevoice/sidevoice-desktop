//! The macOS body: MediaPlayer's Now Playing and remote commands, and the AirPods mute gesture (`apple.rs`), with what
//! only macOS has: the playback state the app sets, and the mute gesture's handler block.
use super::{CallState, CommandSink, Os};
use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_avf_audio::AVAudioApplication;
use objc2_media_player::{MPNowPlayingInfoCenter, MPNowPlayingPlaybackState};
use tauri::AppHandle;

#[path = "apple.rs"]
mod apple;

pub(super) struct Native;

impl Os for Native {
    const NATIVE: bool = true;

    fn attach(app: &AppHandle, sink: CommandSink) {
        apple::attach::<Native>(app, sink)
    }

    fn update(app: &AppHandle, call: &CallState) {
        apple::update::<Native>(app, call)
    }
}

impl apple::Os for Native {
    /// A headset's double press of play/pause arrives as the next-track command.
    const NEXT_TRACK: bool = true;

    /// Apple: on macOS this must be set whenever playback starts or stops, or remote commands may not arrive.
    fn set_playback_state(center: &MPNowPlayingInfoCenter, state: MPNowPlayingPlaybackState) {
        // SAFETY: main thread (`apple::update` dispatches there); a plain property set.
        unsafe { center.setPlaybackState(state) }
    }

    /// The gesture reaches a handler block the app gives `AVAudioApplication` (macOS only), which answers whether the
    /// microphone is now as asked.
    fn listen_for_mute_gesture(app: &AppHandle) -> bool {
        let app = app.clone();
        let handler = RcBlock::new(move |muted: Bool| -> Bool { Bool::new(apple::heard(&app, muted.as_bool())) });
        // SAFETY: the caller checked that the class exists; the system retains the block.
        unsafe { AVAudioApplication::sharedInstance().setInputMuteStateChangeHandler_error(Some(&handler)).is_ok() }
    }

    fn stop_listening_for_mute_gesture() {
        // SAFETY: as above.
        let _ = unsafe { AVAudioApplication::sharedInstance().setInputMuteStateChangeHandler_error(None) };
    }
}
