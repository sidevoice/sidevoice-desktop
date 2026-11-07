//! iOS: an `AVAudioPlayer` on the cue's WAV, looping, in the audio session the native call has made active (the app's
//! play-and-record voice-chat session, `call::CALL_AUDIO`): it joins that session and changes nothing of it. The player
//! lives on the main thread, where the commands run it (`lib.rs` `cue`).
use std::cell::RefCell;

use objc2::{rc::Retained, AnyThread};
use objc2_avf_audio::AVAudioPlayer;
use objc2_foundation::NSData;

use super::Os;
use crate::Cue;

pub struct Native;

thread_local! {
    /// The playing cue's player, on the main thread.
    static PLAYER: RefCell<Option<Retained<AVAudioPlayer>>> = const { RefCell::new(None) };
}

impl Os for Native {
    fn available() -> bool {
        true
    }

    fn start(_: Cue, wav: &[u8]) -> Result<(), String> {
        PLAYER.with(|slot| {
            let mut slot = slot.borrow_mut();
            // SAFETY: `isPlaying` on a live player, on the main thread that made it.
            if slot
                .as_ref()
                .is_some_and(|player| unsafe { player.isPlaying() })
            {
                return Ok(());
            }
            let data = NSData::with_bytes(wav);
            // SAFETY: a fresh player on a WAV the app made; every call below is on it, on the main thread.
            let player =
                unsafe { AVAudioPlayer::initWithData_error(AVAudioPlayer::alloc(), &data) }
                    .map_err(|error| error.localizedDescription().to_string())?;
            unsafe {
                // A negative count loops until stopped.
                player.setNumberOfLoops(-1);
                player.setVolume(1.0);
                player.prepareToPlay();
                if !player.play() {
                    return Err("cue-play-refused".into());
                }
            }
            *slot = Some(player);
            Ok(())
        })
    }

    fn stop(_: Cue) {
        PLAYER.with(|slot| {
            if let Some(player) = slot.borrow_mut().take() {
                // SAFETY: `stop` on a live player, on the main thread that made it.
                unsafe { player.stop() };
            }
        });
    }
}
