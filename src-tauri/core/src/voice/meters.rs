//! The call's two meters on their way to the room page: the microphone's level (`VoiceEvent::Level`) and the reply's
//! output level (`VoiceEvent::PlaybackLevel`), each 0–1, which sidevoice-voice publishes once per detector window
//! (512 samples at 16 kHz: about 31 a second each, silence included). Each one handed over is a `webview.eval` on the
//! app's main thread and a script run in the page, so only those the page would draw differently go (desktop#149):
//!
//! - the level, when its percent changes (the page draws it in whole percents: `--mic-fill`, the card's wave);
//! - the output level, unless it is silence again: the first 0 closes the mouth, the next ones change nothing. A level
//!   that sounds goes every time, so the page's "no sample for 300 ms is silence" still holds while a reply plays.

#[cfg(test)]
mod tests;

/// What the page was last given, per meter.
#[derive(Debug, Default)]
pub struct Meters {
    level: Option<u8>,
    playback_silent: bool,
}

impl Meters {
    /// Whether the microphone's level `value` goes to the page: its percent is not the last one sent.
    pub fn level(&mut self, value: f32) -> bool {
        let percent = (value.clamp(0.0, 1.0) * 100.0).round() as u8;
        self.level.replace(percent) != Some(percent)
    }

    /// Whether the reply's output level `value` goes to the page: anything but a silence that follows a silence.
    pub fn playback(&mut self, value: f32) -> bool {
        let silent = value.is_nan() || value <= 0.0;
        let was = std::mem::replace(&mut self.playback_silent, silent);
        !(silent && was)
    }
}
