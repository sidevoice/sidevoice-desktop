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
    /// A headset's play/pause reaches the Now Playing app as a media command, also during a call.
    const NOW_PLAYING: bool = true;

    /// A headset's double press of play/pause arrives as the next-track command.
    const NEXT_TRACK: bool = true;

    /// Apple: on macOS this must be set whenever playback starts or stops, or remote commands may not arrive.
    fn set_playback_state(center: &MPNowPlayingInfoCenter, state: MPNowPlayingPlaybackState) {
        // SAFETY: main thread (`apple::update` dispatches there); a plain property set.
        unsafe { center.setPlaybackState(state) }
    }

    /// The gesture reaches a handler block the app gives `AVAudioApplication` (macOS only), which answers whether the
    /// microphone is now as asked. The handler alone is not the opt-in: the audio server gives the gesture only to a recording
    /// app that observes the input-mute notification, and refuses it otherwise ("Cannot Control Mic with AirPods"). So the
    /// notification is observed too, and does nothing more: the handler already carries the state, and the notification
    /// that follows it would be the same gesture a second time.
    fn listen_for_mute_gesture(app: &AppHandle) -> bool {
        let app = app.clone();
        let handler = RcBlock::new(move |muted: Bool| -> Bool { Bool::new(apple::heard(&app, muted.as_bool())) });
        // SAFETY: the caller checked that the class exists; the system retains the block.
        let handled = unsafe {
            AVAudioApplication::sharedInstance().setInputMuteStateChangeHandler_error(Some(&handler)).is_ok()
        };
        let opted_in = apple::observe_input_mute(|_| {});
        handled && opted_in
    }
}
