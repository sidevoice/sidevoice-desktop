//! The page's cues (`crate::cue`): each OS file implements `Os`.
use crate::Cue;

#[cfg_attr(target_os = "android", path = "cue/android.rs")]
#[cfg_attr(target_os = "ios", path = "cue/ios.rs")]
mod imp;
use imp::Native;

trait Os {
    /// Whether this phone plays the page's cues itself.
    fn available() -> bool;
    /// A sound's file (AAC in MP4) as mono samples of full scale ±1, and their rate.
    fn decode(m4a: &[u8]) -> Result<(Vec<f32>, u32), String>;
    /// Plays the cue's `wav` (a WAV file) as a loop until `stop`; one cue at a time. Starting what plays (the same `key`)
    /// changes nothing; something else replaces it. Call on the main thread.
    fn start(cue: Cue, key: &str, wav: &[u8]) -> Result<(), String>;
    /// Stops it; nothing when it does not play. Call on the main thread.
    fn stop(cue: Cue);
}

pub fn available() -> bool {
    Native::available()
}

pub fn decode(m4a: &[u8]) -> Result<(Vec<f32>, u32), String> {
    Native::decode(m4a)
}

pub fn start(cue: Cue, key: &str, wav: &[u8]) -> Result<(), String> {
    Native::start(cue, key, wav)
}

pub fn stop(cue: Cue) {
    Native::stop(cue);
}
