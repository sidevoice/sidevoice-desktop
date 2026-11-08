//! The page's cues, played by the app (`host.cue`, docs/BRIDGE.md -> "The cues"): where the webview's Web Audio is not sure to
//! come out through the audio session the native call holds (sidevoice-web#439), the page says when and the app plays the
//! sound through that same session. Each phone asks its own system (D-HOST-OS); a phone with no player says so
//! (`available`), and the page then plays the cue itself.
//!
//! The sound is the one the person chose in Settings, from the web the app bundles: its manifest ([`SOUNDS_MANIFEST`]) says
//! which sounds there are, the default, each one's cadence and file, and the file is read from the app's own assets, so the
//! sounds live in the web alone. The app plays it as the page would: the sound, moved by the person's offset and limited as
//! the page's limiter does, then a silence drawn in its interval, again and again ([`render`]).
use crate::platform::cue as platform;

/// The cues the page may ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// The sound that says the agent is working on the person's turn.
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

/// The thinking sounds' manifest in the app's assets: the bundled web's `sounds/thinking/sounds.json` (its
/// `apps/web/public/`), under `voice/`, where the app serves the web.
pub const SOUNDS_MANIFEST: &str = "voice/sounds/thinking/sounds.json";
const SOUNDS_FOLDER: &str = "voice/sounds/thinking/";

/// The longest silence between plays, and the longest sound, and the rates, the app takes (seconds; Hz): far beyond the web's
/// sounds (6 s at most, a gap of 12), and a bound on what the loop holds in memory (2 plays and gaps at most past 20 s).
const MAX_GAP_SECONDS: f32 = 30.0;
const MAX_SOUND_SECONDS: f32 = 15.0;
const RATES: std::ops::RangeInclusive<u32> = 8_000..=96_000;

/// The person's offset from the sound's own level, in dB: the page's range (services/thinking-sounds.ts).
const GAIN_DB: (f32, f32) = (-20.0, 10.0);

/// What the manifest says; only what the app reads of it.
#[derive(Debug, Clone, serde::Deserialize)]
struct Manifest {
    default: String,
    sounds: Vec<Sound>,
}

/// One thinking sound: its id, how long the silence after each play may be (seconds, `[min, max]`), and its files.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct Sound {
    pub id: String,
    pub interval: [f32; 2],
    files: Files,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
struct Files {
    /// AAC in MP4: what the phone's player decodes (the web has Opus too, for the page).
    m4a: String,
}

impl Sound {
    /// Its file in the app's assets.
    pub fn asset(&self) -> String {
        format!("{SOUNDS_FOLDER}{}", self.files.m4a)
    }
}

/// The sound a start asks for (`sound`, an id of the manifest; none: its default), with its file read through `read` (the
/// app's asset resolver). Refused when the manifest is missing or not one, the id unknown, or the file not a sound.
pub fn chosen_sound(
    read: impl Fn(&str) -> Option<Vec<u8>>,
    sound: Option<&str>,
) -> Result<(Sound, Vec<u8>), String> {
    let manifest = read(SOUNDS_MANIFEST).ok_or("cue-sounds-missing")?;
    let manifest: Manifest =
        serde_json::from_slice(&manifest).map_err(|_| "cue-sounds-unreadable".to_string())?;
    let id = sound.unwrap_or(&manifest.default);
    let found = manifest
        .sounds
        .into_iter()
        .find(|each| each.id == id)
        .ok_or_else(|| format!("cue-sound-unknown: {id}"))?;
    let [min, max] = found.interval;
    let plain =
        |name: &str| !name.is_empty() && !name.contains(['/', '\\']) && !name.contains("..");
    if !(min.is_finite() && max.is_finite() && min >= 0.0 && max >= min && max <= MAX_GAP_SECONDS)
        || !plain(&found.files.m4a)
    {
        return Err(format!("cue-sound-malformed: {id}"));
    }
    let bytes = read(&found.asset()).ok_or_else(|| format!("cue-sound-missing: {id}"))?;
    // A path the resolver does not have is answered with the site's index.html: only an MP4 is the sound.
    if !is_mp4(&bytes) {
        return Err(format!("cue-sound-missing: {id}"));
    }
    Ok((found, bytes))
}

/// Whether `bytes` are an MP4 file: its first box is `ftyp`.
pub fn is_mp4(bytes: &[u8]) -> bool {
    bytes.len() > 8 && &bytes[4..8] == b"ftyp"
}

/// The person's offset, as a start gives it: none is the sound as it is (0 dB, an older page); a number is kept within the
/// page's range, whole dB; anything else is refused.
pub fn gain_db(value: Option<f64>) -> Result<f32, String> {
    match value {
        None => Ok(0.0),
        Some(db) if db.is_finite() => Ok((db.round() as f32).clamp(GAIN_DB.0, GAIN_DB.1)),
        Some(_) => Err("cue takes gainDb as a number of dB".into()),
    }
}

/// Which start may still play. Each start is asked for with a ticket of its own ([`cue_ticket`]); a later start, the page's stop,
/// every call end and page teardown move it on ([`end_cues`]), and a start whose ticket is past is a no-op ([`start_cue`]): the
/// sound is read and decoded between the request and the play, and what was asked for after it wins.
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The ticket of a start asked for now: every start asked for before it is stale from now on.
pub fn cue_ticket() -> u64 {
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
}

/// Whether the start asked for with `ticket` is still the last thing asked for: no later start, stop or end.
pub fn cue_current(ticket: u64) -> bool {
    GENERATION.load(std::sync::atomic::Ordering::SeqCst) == ticket
}

/// The page stopped the cue, the call is over, or the page is gone: every start asked for before is stale. Does not stop the
/// player: [`stop_cue`] does, on the main thread, in order after the starts already queued there.
pub fn end_cues() {
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// Whether this phone plays the page's cues itself: it has a player, and the web it bundles has the default sound (`read`, the
/// app's asset resolver). An app whose web has no sounds offers none, and the page plays its own.
pub fn cue_available(read: impl Fn(&str) -> Option<Vec<u8>>) -> bool {
    platform::available() && chosen_sound(read, None).is_ok()
}

/// What a start plays: `sound` at `gain_db`, as [`render`] makes it, keyed by both so that a start of what already plays is
/// told from one of something else.
pub struct Prepared {
    pub key: String,
    pub wav: Vec<u8>,
}

/// A sound decoded: its samples and their rate.
type Decoded = std::sync::Arc<(Vec<f32>, u32)>;
/// The decoded sound last played, by id: decoding it again for every turn of the agent's work is not needed.
static DECODED: std::sync::Mutex<Option<(String, Decoded)>> = std::sync::Mutex::new(None);

/// Reads, decodes and renders the sound a start asks for (off the main thread: it decodes). `seed` draws its silences.
pub fn prepare_cue(
    read: impl Fn(&str) -> Option<Vec<u8>>,
    sound: Option<&str>,
    gain_db: f32,
    seed: u32,
) -> Result<Prepared, String> {
    let (sound, file) = chosen_sound(read, sound)?;
    let cached = DECODED
        .lock()
        .unwrap()
        .as_ref()
        .filter(|(id, _)| *id == sound.id)
        .map(|(_, pcm)| pcm.clone());
    let pcm = match cached {
        Some(pcm) => pcm,
        None => {
            let decoded = std::sync::Arc::new(platform::decode(&file)?);
            *DECODED.lock().unwrap() = Some((sound.id.clone(), decoded.clone()));
            decoded
        }
    };
    let (samples, rate) = (&pcm.0, pcm.1);
    Ok(Prepared {
        key: format!("{}@{gain_db}", sound.id),
        wav: render(samples, rate, sound.interval, gain_db, seed)?,
    })
}

/// Plays `cue` until [`stop_cue`]: a start of what already plays (the same `key`) changes nothing, of something else replaces
/// it. On the main thread. Nothing if `ticket` (from [`cue_ticket`], when the start was asked for) is stale: the page stopped
/// it, asked for another, or the call ended in between. A stop or an end asked for once this check has passed moves the ticket
/// on and then queues its [`stop_cue`] on this same thread, behind this start: what it plays is stopped right after.
pub fn start_cue(cue: Cue, prepared: &Prepared, ticket: u64) -> Result<(), String> {
    if !cue_current(ticket) {
        return Ok(());
    }
    platform::start(cue, &prepared.key, &prepared.wav)
}

/// Stops it; nothing when it does not play. On the main thread.
pub fn stop_cue(cue: Cue) {
    platform::stop(cue);
}

/// The page's limiter (services/thinking-sound.ts `LIMITER`, a Web Audio compressor): a threshold just under full scale, a
/// hard knee, a high ratio, quick to act and to let go.
const THRESHOLD_DB: f32 = -2.0;
const RATIO: f32 = 20.0;
const ATTACK_SECONDS: f32 = 0.003;
const RELEASE_SECONDS: f32 = 0.1;
/// The level a Web Audio compressor adds back for what it may take off (its makeup: 0.6 of what full scale loses), so that the
/// app's sound is as loud as the page's.
const MAKEUP_DB: f32 = 0.6 * (-THRESHOLD_DB - -THRESHOLD_DB / RATIO);
/// How long the loop the player repeats lasts at least, in whole plays, and never fewer than two: its silences differ within
/// it, as the page's do.
const LOOP_SECONDS: f32 = 20.0;

fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// `pcm` moved by `gain_db` and through the limiter: what the page plays of one play of the sound. Above the threshold a
/// sample keeps what the ratio leaves of it; ahead of it the gain comes down over the attack (the page's compressor looks ahead
/// too), after it goes back up over the release, and it is never more than a sample allows: a raised sound does not clip.
fn shape(pcm: &[f32], rate: u32, gain_db: f32) -> Vec<f32> {
    let (gain, makeup) = (db_to_gain(gain_db), db_to_gain(MAKEUP_DB));
    let attack = (-1.0 / (ATTACK_SECONDS * rate as f32)).exp();
    let release = (-1.0 / (RELEASE_SECONDS * rate as f32)).exp();
    let raised: Vec<f32> = pcm.iter().map(|sample| sample * gain).collect();
    let mut reduction: Vec<f32> = raised
        .iter()
        .map(|sample| {
            let level = 20.0 * sample.abs().max(1e-9).log10();
            if level > THRESHOLD_DB {
                db_to_gain(THRESHOLD_DB + (level - THRESHOLD_DB) / RATIO - level)
            } else {
                1.0
            }
        })
        .collect();
    for at in (1..reduction.len()).rev() {
        reduction[at - 1] = reduction[at - 1].min(1.0 - (1.0 - reduction[at]) * attack);
    }
    for at in 1..reduction.len() {
        reduction[at] = reduction[at].min(1.0 - (1.0 - reduction[at - 1]) * release);
    }
    raised
        .iter()
        .zip(&reduction)
        .map(|(sample, reduction)| (sample * reduction * makeup).clamp(-1.0, 1.0))
        .collect()
}

/// The loop the player repeats, as a mono 16-bit WAV at `rate`: the sound at the person's offset, then a silence drawn in its
/// `interval`, again, for at least [`LOOP_SECONDS`]. It starts with the sound, as the page plays one at once. Refused for a
/// sound with no samples, longer than [`MAX_SOUND_SECONDS`] or at a rate out of [`RATES`], or a gap past [`MAX_GAP_SECONDS`]:
/// what the loop holds stays bounded.
pub fn render(
    pcm: &[f32],
    rate: u32,
    interval: [f32; 2],
    gain_db: f32,
    seed: u32,
) -> Result<Vec<u8>, String> {
    let [min, max] = interval;
    if pcm.is_empty()
        || !RATES.contains(&rate)
        || pcm.len() as f32 > MAX_SOUND_SECONDS * rate as f32
        || !(min >= 0.0 && max >= min && max <= MAX_GAP_SECONDS)
    {
        return Err("cue-sound-unplayable".into());
    }
    let play = shape(pcm, rate, gain_db);
    let mut seed = seed | 1;
    let mut samples: Vec<i16> = Vec::new();
    let mut plays = 0;
    while plays < 2 || (samples.len() as f32) < LOOP_SECONDS * rate as f32 {
        samples.extend(play.iter().map(|s| (s * f32::from(i16::MAX)) as i16));
        // xorshift: a gap anywhere in the interval.
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let gap = interval[0] + (seed as f32 / u32::MAX as f32) * (interval[1] - interval[0]);
        samples.extend(std::iter::repeat_n(0, (gap * rate as f32) as usize));
        plays += 1;
    }
    Ok(wav(&samples, rate))
}

fn wav(samples: &[i16], rate: u32) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend(b"RIFF");
    out.extend((36 + data).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16_u32.to_le_bytes());
    out.extend(1_u16.to_le_bytes()); // PCM
    out.extend(1_u16.to_le_bytes()); // mono
    out.extend(rate.to_le_bytes());
    out.extend((rate * 2).to_le_bytes());
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

    const MANIFEST: &str = r#"{
      "default": "soft-heartbeat",
      "sounds": [
        {"id":"soft-heartbeat","interval":[4,6],"files":{"webm":"soft-heartbeat.webm","m4a":"soft-heartbeat.m4a"}},
        {"id":"single-beat","interval":[2,3],"files":{"webm":"single-beat.webm","m4a":"single-beat.m4a"}}
      ]
    }"#;
    const M4A: &[u8] = b"\0\0\0\x18ftypM4A \0\0\x02\0isomiso2";
    const INDEX: &[u8] = b"<!doctype html><meta http-equiv=\"refresh\" content=\"0; url=/voice/\">";

    /// The app's assets as Tauri's resolver answers them: a path it does not have is the site's index.html.
    fn assets(files: &'static [(&'static str, &'static [u8])]) -> impl Fn(&str) -> Option<Vec<u8>> {
        move |path| {
            Some(
                files
                    .iter()
                    .find(|(name, _)| *name == path)
                    .map_or(INDEX, |(_, bytes)| bytes)
                    .to_vec(),
            )
        }
    }

    const BUNDLED: &[(&str, &[u8])] = &[
        (SOUNDS_MANIFEST, MANIFEST.as_bytes()),
        ("voice/sounds/thinking/soft-heartbeat.m4a", M4A),
        ("voice/sounds/thinking/single-beat.m4a", M4A),
    ];

    #[test]
    fn only_the_thinking_cue_is_known() {
        assert_eq!(Cue::named("thinking"), Some(Cue::Thinking));
        assert_eq!(Cue::named("ring"), None);
        assert_eq!(Cue::named(""), None);
    }

    #[test]
    fn a_start_finds_the_chosen_sound_in_the_bundled_web_and_no_sound_is_the_default() {
        let (sound, bytes) = chosen_sound(assets(BUNDLED), Some("single-beat")).unwrap();
        assert_eq!(sound.id, "single-beat");
        assert_eq!(sound.interval, [2.0, 3.0]);
        assert_eq!(sound.asset(), "voice/sounds/thinking/single-beat.m4a");
        assert_eq!(bytes, M4A);
        // An older page's start("thinking"): the default.
        assert_eq!(
            chosen_sound(assets(BUNDLED), None).unwrap().0.id,
            "soft-heartbeat"
        );
    }

    #[test]
    fn a_sound_the_web_does_not_have_is_not_played() {
        assert!(chosen_sound(assets(BUNDLED), Some("breath"))
            .unwrap_err()
            .starts_with("cue-sound-unknown"));
        // The manifest names it, the file is not there: the resolver's index.html is no sound.
        const NO_FILE: &[(&str, &[u8])] = &[(SOUNDS_MANIFEST, MANIFEST.as_bytes())];
        assert!(chosen_sound(assets(NO_FILE), None)
            .unwrap_err()
            .starts_with("cue-sound-missing"));
        // A web with no sounds at all (one before them): its index.html is no manifest, and the app offers no player.
        assert_eq!(
            chosen_sound(assets(&[]), None).unwrap_err(),
            "cue-sounds-unreadable"
        );
        assert!(!cue_available(assets(&[])));
        assert_eq!(
            chosen_sound(|_| None, None).unwrap_err(),
            "cue-sounds-missing"
        );
    }

    #[test]
    fn a_file_name_that_leaves_the_sounds_folder_is_refused() {
        const OUT: &str = r#"{"default":"x","sounds":[{"id":"x","interval":[1,2],"files":{"m4a":"../index.html"}}]}"#;
        const BAD: &[(&str, &[u8])] = &[(SOUNDS_MANIFEST, OUT.as_bytes())];
        assert!(chosen_sound(assets(BAD), None)
            .unwrap_err()
            .starts_with("cue-sound-malformed"));
    }

    #[test]
    fn the_offset_is_whole_db_in_the_pages_range_and_none_is_the_sound_as_it_is() {
        assert_eq!(gain_db(None), Ok(0.0));
        assert_eq!(gain_db(Some(-6.4)), Ok(-6.0));
        assert_eq!(gain_db(Some(40.0)), Ok(10.0));
        assert_eq!(gain_db(Some(-99.0)), Ok(-20.0));
        assert!(gain_db(Some(f64::NAN)).is_err());
        assert!(gain_db(Some(f64::INFINITY)).is_err());
    }

    #[test]
    fn a_start_is_stale_once_anything_is_asked_for_after_it() {
        let prepared = Prepared {
            key: "soft-heartbeat@0".into(),
            wav: Vec::new(),
        };
        let asked = cue_ticket();
        assert!(cue_current(asked), "nothing since: it may play");
        // A call ended (or the page went) between the request and its run.
        end_cues();
        assert!(!cue_current(asked));
        // Past it, the start is a no-op (it never reaches the player), whatever the OS.
        assert_eq!(start_cue(Cue::Thinking, &prepared, asked), Ok(()));
        // A later start wins over an earlier one still being read; the page's stop (an end) over both.
        let first = cue_ticket();
        let second = cue_ticket();
        assert!(!cue_current(first) && cue_current(second));
        end_cues();
        assert!(!cue_current(second));
    }

    fn samples(wav: &[u8]) -> Vec<i16> {
        (44..wav.len())
            .step_by(2)
            .map(|at| i16::from_le_bytes([wav[at], wav[at + 1]]))
            .collect()
    }

    /// One play: a 0.5 s low tone at `peak` of full scale.
    fn tone(rate: u32, peak: f32) -> Vec<f32> {
        (0..rate / 2)
            .map(|i| peak * (2.0 * std::f32::consts::PI * 60.0 * i as f32 / rate as f32).sin())
            .collect()
    }

    #[test]
    fn the_loop_is_the_sound_then_a_silence_in_its_interval_again_and_again() {
        let rate = 8_000;
        let play = tone(rate, 0.1);
        let wav = render(&play, rate, [4.0, 6.0], 0.0, 7).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), rate);
        assert_eq!(
            u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize,
            wav.len() - 8
        );
        let all = samples(&wav);
        assert!(all.len() as f32 >= LOOP_SECONDS * rate as f32);
        // It starts with the sound, and every sound is followed by a silence within the interval.
        assert_ne!(all[1], 0, "the first play at once");
        let mut at = 0;
        let mut plays = 0;
        while at < all.len() {
            at += play.len();
            let silence = all[at..].iter().take_while(|s| **s == 0).count();
            let seconds = silence as f32 / rate as f32;
            assert!(
                (4.0..=6.0).contains(&seconds),
                "a gap of {seconds} s after play {plays}"
            );
            at += silence;
            plays += 1;
        }
        assert!(plays >= 2);
        // Another seed, other silences: they are drawn, not fixed.
        assert_ne!(render(&play, rate, [4.0, 6.0], 0.0, 8).unwrap(), wav);
    }

    #[test]
    fn a_sound_that_would_not_end_or_would_not_fit_is_refused() {
        let rate = 8_000;
        let play = tone(rate, 0.1);
        // No samples (a file that decodes to nothing) would never fill the loop.
        assert!(render(&[], rate, [0.0, 0.0], 0.0, 1).is_err());
        assert!(render(&play, 0, [1.0, 2.0], 0.0, 1).is_err());
        assert!(render(&play, rate, [1.0, 1e30], 0.0, 1).is_err());
        assert!(render(&play, rate, [f32::NAN, 2.0], 0.0, 1).is_err());
        assert!(render(&tone(rate, 0.1).repeat(31), rate, [1.0, 2.0], 0.0, 1).is_err());
        const LONG: &str =
            r#"{"default":"x","sounds":[{"id":"x","interval":[1,1e30],"files":{"m4a":"x.m4a"}}]}"#;
        const BAD: &[(&str, &[u8])] = &[(SOUNDS_MANIFEST, LONG.as_bytes())];
        assert!(chosen_sound(assets(BAD), None)
            .unwrap_err()
            .starts_with("cue-sound-malformed"));
    }

    #[test]
    fn the_offset_moves_the_level_and_the_limiter_keeps_a_raised_sound_from_clipping() {
        let rate = 8_000;
        let peak = |gain_db: f32, of: f32| {
            let all = samples(&render(&tone(rate, of), rate, [1.0, 1.0], gain_db, 1).unwrap());
            all.iter().map(|s| s.unsigned_abs()).max().unwrap() as f32 / f32::from(i16::MAX)
        };
        // Beneath the threshold, as the page plays it: the offset and the compressor's makeup, nothing else.
        let quiet = peak(0.0, 0.1);
        assert!(
            (quiet - 0.1 * db_to_gain(MAKEUP_DB)).abs() < 0.002,
            "{quiet}"
        );
        let lowered = peak(-20.0, 0.1);
        assert!((lowered / quiet - 0.1).abs() < 0.01, "{lowered}");
        // A sound near full scale raised by 10 dB is held under it.
        let raised = peak(10.0, 0.89);
        assert!(raised < 1.0 && raised > 0.8, "{raised}");
        assert!((MAKEUP_DB - 1.14).abs() < 0.01);
    }
}
