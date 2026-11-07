//! The page's cues, played by the app (`host.cue`, docs/BRIDGE.md -> "The cues"): where the webview's Web Audio is not sure to
//! come out through the audio session the native call holds (sidevoice-web#439), the page says when and the app plays the
//! sound through that same session. Each phone asks its own system (D-HOST-OS); a phone with no player says so
//! (`available`), and the page then plays the cue itself.
use crate::platform::cue as platform;

/// The cues the page may ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// The soft breath that says the agent is working on the person's turn.
    Thinking,
}

impl Cue {
    pub fn named(name: &str) -> Option<Cue> {
        match name {
            "thinking" => Some(Cue::Thinking),
            _ => None,
        }
    }
}

/// Whether this phone plays the page's cues itself.
pub fn cue_available() -> bool {
    platform::available()
}

/// Plays `cue` as a loop until [`stop_cue`]; starting one that plays changes nothing. On the main thread.
pub fn start_cue(cue: Cue) -> Result<(), String> {
    platform::start(cue, &wav(cue))
}

/// Stops it; nothing when it does not play. On the main thread.
pub fn stop_cue(cue: Cue) {
    platform::stop(cue);
}

const RATE: u32 = 22_050;
/// One breath, the page's own length (`services/thinking-sound.ts`, `BREATH_SECONDS`): the loop is whole breaths.
const BREATH_SECONDS: f32 = 3.2;
/// The breath's peak, of full scale. The page's own (0.035 of a noise a low-pass leaves a third of) is about -44 dBFS: a
/// phone's loudspeaker in a call does not carry that. Here the loop is audible at the call's volume and still far
/// beneath the voice.
const PEAK: f32 = 0.5;

/// The cue as a mono 16-bit WAV that loops without a seam: low-passed noise (700 Hz, as the page's) under a slow swell
/// that is silent at both ends of the file, one breath long.
fn wav(cue: Cue) -> Vec<u8> {
    let Cue::Thinking = cue;
    let count = (RATE as f32 * BREATH_SECONDS) as usize;
    let alpha = 1.0 - (-2.0 * std::f32::consts::PI * 700.0 / RATE as f32).exp();
    let (mut seed, mut low) = (0x9E37_79B9_u32, 0.0_f32);
    let samples: Vec<i16> = (0..count)
        .map(|i| {
            // xorshift: the same breath every time.
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = seed as f32 / u32::MAX as f32 * 2.0 - 1.0;
            low += alpha * (noise - low);
            let swell = (1.0 - (2.0 * std::f32::consts::PI * i as f32 / count as f32).cos()) / 2.0;
            // The low-pass leaves about a third of the noise's size: make most of it up, and never clip.
            ((low * 2.0 * swell * PEAK).clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
        })
        .collect();
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend(b"RIFF");
    out.extend((36 + data).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16_u32.to_le_bytes());
    out.extend(1_u16.to_le_bytes()); // PCM
    out.extend(1_u16.to_le_bytes()); // mono
    out.extend(RATE.to_le_bytes());
    out.extend((RATE * 2).to_le_bytes());
    out.extend(2_u16.to_le_bytes());
    out.extend(16_u16.to_le_bytes());
    out.extend(b"data");
    out.extend(data.to_le_bytes());
    for sample in samples {
        out.extend(sample.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_thinking_cue_is_known() {
        assert_eq!(Cue::named("thinking"), Some(Cue::Thinking));
        assert_eq!(Cue::named("ring"), None);
        assert_eq!(Cue::named(""), None);
    }

    #[test]
    fn the_breath_is_a_whole_loop_of_a_wav_that_starts_and_ends_in_silence() {
        let wav = wav(Cue::Thinking);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        let samples: Vec<i16> = (44..wav.len())
            .step_by(2)
            .map(|at| i16::from_le_bytes([wav[at], wav[at + 1]]))
            .collect();
        assert_eq!(samples.len(), (RATE as f32 * BREATH_SECONDS) as usize);
        assert_eq!(
            u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize,
            wav.len() - 8
        );
        assert!(
            samples[0].abs() < 8 && samples[samples.len() - 1].abs() < 64,
            "no click at the seam"
        );
        let loudest = samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(loudest > 1_000, "audible: {loudest}");
        assert!(
            f32::from(loudest) < f32::from(i16::MAX) * 0.9,
            "far from clipping: {loudest}"
        );
    }

    #[test]
    fn the_breath_is_the_same_every_time() {
        assert_eq!(wav(Cue::Thinking), wav(Cue::Thinking));
    }
}
