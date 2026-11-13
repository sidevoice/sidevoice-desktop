//! iOS: the shared `AVAudioSession`'s current route and `overrideOutputAudioPort(.speaker)`, valid for a play-and-record
//! session, which the call's is (`call::CALL_AUDIO`). The session answers from any thread.
use objc2_avf_audio::{
    AVAudioSession, AVAudioSessionPortBuiltInReceiver, AVAudioSessionPortBuiltInSpeaker,
    AVAudioSessionPortOverride,
};

use super::Os;

pub struct Native;

impl Os for Native {
    fn on_the_phone() -> bool {
        // SAFETY: read-only messages to the shared session, reads of a live port description, and constants of AVFAudio,
        // linked by the app.
        unsafe {
            let outputs = AVAudioSession::sharedInstance().currentRoute().outputs();
            let Some(port) = outputs.iter().next() else {
                return false;
            };
            let kind = port.portType();
            [
                AVAudioSessionPortBuiltInReceiver,
                AVAudioSessionPortBuiltInSpeaker,
            ]
            .into_iter()
            .flatten()
            .any(|own| kind.isEqualToString(own))
        }
    }

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
