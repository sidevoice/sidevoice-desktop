//! The phone's own loudspeaker for the call's audio. The call declares play-and-record with "the loudspeaker rather than
//! the receiver" (`call::CALL_AUDIO`), yet iOS can still play an active play-and-record session on the receiver (CallKit's
//! audio call) or on the loudspeaker at a far lower level than the media volume (the same session, the same declaration:
//! reported to Apple since iOS 13, developer forums thread 721535), with the volume keys moving it only a little. Only
//! `overrideOutputAudioPort(.speaker)` on the active session gives the loudspeaker its full level, and it lasts until the
//! route changes. So, once the session is active, a call that plays on the phone itself is moved to the loudspeaker; one
//! that plays on headphones, a car or AirPlay is left where the system put it. Android routes the call itself (its call
//! service, Telecom) and has nothing to do here.
#[cfg_attr(target_os = "android", path = "loudspeaker/android.rs")]
#[cfg_attr(target_os = "ios", path = "loudspeaker/ios.rs")]
mod imp;
use imp::Native;

/// Where the active session plays now, as far as the choice needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Output {
    /// The speaker held to the ear.
    Receiver,
    /// The phone's loudspeaker.
    Loudspeaker,
    /// Anything else (headphones, Bluetooth, a car, AirPlay), or a route the system does not say.
    Other,
}

/// What differs per OS.
trait Os {
    /// Where the audio plays now.
    fn output() -> Output;
    /// Has the system play the active session on the loudspeaker until the route changes.
    fn force() -> Result<(), String>;
}

/// Whether a session playing on `output` is moved to the loudspeaker: only what plays on the phone itself.
fn moves(output: Output) -> bool {
    matches!(output, Output::Receiver | Output::Loudspeaker)
}

/// Once the call's session is active: a call that plays on the phone itself plays on its loudspeaker, at its full level.
/// A refusal is logged; the call goes on where the system put it.
pub fn take() {
    let output = Native::output();
    if !moves(output) {
        return;
    }
    match Native::force() {
        Ok(()) => log::info!("call: the audio plays on the loudspeaker (it was on {output:?})"),
        Err(error) => log::warn!("call: the audio stays on {output:?}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{moves, Output};

    #[test]
    fn only_audio_on_the_phone_itself_moves_to_the_loudspeaker() {
        assert!(
            moves(Output::Receiver),
            "the receiver is what makes the call sound so low"
        );
        assert!(
            moves(Output::Loudspeaker),
            "the loudspeaker, again, for its full level"
        );
        assert!(
            !moves(Output::Other),
            "headphones, a car or AirPlay stay as the system chose"
        );
    }
}
