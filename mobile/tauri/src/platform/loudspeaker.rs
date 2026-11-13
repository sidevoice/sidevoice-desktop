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

/// What differs per OS.
trait Os {
    /// Whether the active session plays on the phone itself now (its receiver or its loudspeaker), rather than on
    /// headphones, Bluetooth, a car or AirPlay. False where the OS routes the call itself.
    fn on_the_phone() -> bool;
    /// Has the system play the active session on the loudspeaker until the route changes.
    fn force() -> Result<(), String>;
}

/// Once the call's session is active: a call that plays on the phone itself plays on its loudspeaker, at its full level.
/// A refusal is logged; the call goes on where the system put it.
pub fn take() {
    if !Native::on_the_phone() {
        return;
    }
    match Native::force() {
        Ok(()) => log::info!("call: the audio plays on the phone's loudspeaker"),
        Err(error) => log::warn!("call: the audio stays where the system put it: {error}"),
    }
}
