//! The page's cues (`crate::cue`): each OS file implements `Os`.
use crate::Cue;

#[cfg_attr(target_os = "android", path = "cue/android.rs")]
#[cfg_attr(target_os = "ios", path = "cue/ios.rs")]
mod imp;
use imp::Native;

trait Os {
    /// Whether this phone plays the page's cues itself.
    fn available() -> bool;
    /// Plays the cue as a loop until `stop`; one cue at a time, and starting one that plays changes nothing. `wav` is the
    /// cue's sound as a WAV file. Call on the main thread.
    fn start(cue: Cue, wav: &[u8]) -> Result<(), String>;
    /// Stops it; nothing when it does not play. Call on the main thread.
    fn stop(cue: Cue);
}

pub fn available() -> bool {
    Native::available()
}

pub fn start(cue: Cue, wav: &[u8]) -> Result<(), String> {
    Native::start(cue, wav)
}

pub fn stop(cue: Cue) {
    Native::stop(cue);
}
