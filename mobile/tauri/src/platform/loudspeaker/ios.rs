//! iOS: `overrideOutputAudioPort(.speaker)`, valid for a play-and-record
//! session, which the call's is (`call::call_audio`). The session answers from any thread.
use objc2_avf_audio::{AVAudioSession, AVAudioSessionPortOverride};

use super::Os;

pub struct Native;

impl Os for Native {
    fn force() -> Result<(), String> {
        // SAFETY: a message to the shared session with an argument of the type it takes.
        unsafe {
            AVAudioSession::sharedInstance()
                .overrideOutputAudioPort_error(AVAudioSessionPortOverride::Speaker)
        }
        .map_err(|error| {
            format!(
                "the audio session refused the loudspeaker: {} ({})",
                error.localizedDescription(),
                error.code()
            )
        })
    }
}
