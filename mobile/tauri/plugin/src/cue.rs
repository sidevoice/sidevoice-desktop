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

/// The longest silence between plays, and the longest sound, and the rates, the app takes (seconds; Hz): beyond the web's
/// sounds (6 s at most, a gap of 12, 36 at the slowest frequency), and a bound on what the loop holds in memory (2 plays and
/// gaps at most past 20 s: some 7 MB of 16-bit mono at 48 kHz with the longest gaps). A gap the person's frequency would draw
/// longer is drawn at this ([`scaled_interval`]).
const MAX_GAP_SECONDS: f32 = 36.0;
const MAX_SOUND_SECONDS: f32 = 15.0;
const RATES: std::ops::RangeInclusive<u32> = 8_000..=96_000;

/// The person's offset from the sound's own level, in dB: the page's range (services/thinking-sounds.ts).
const GAIN_DB: (f32, f32) = (-20.0, 10.0);

/// How much the person's frequency multiplies the silence after each play: the page's steps run from 0.5 (twice as often) to 3
/// (a third as often; services/thinking-sounds.ts `THINKING_SOUND_INTERVAL_SCALES`).
const INTERVAL_SCALE: (f32, f32) = (0.5, 3.0);

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

/// The person's frequency, as a start gives it (`intervalScale`): a number within the page's range; none, or anything else (not
/// a number, or out of the range), is the sound's own cadence (1).
pub fn interval_scale(value: Option<f64>) -> f32 {
    let (low, high) = INTERVAL_SCALE;
    match value {
        Some(scale) if (f64::from(low)..=f64::from(high)).contains(&scale) => scale as f32,
        _ => 1.0,
    }
}

/// The silence after each play at the person's frequency: the sound's own `interval` times `scale`, as the page draws it
/// (services/thinking-sounds.ts `scaledInterval`), each end at most [`MAX_GAP_SECONDS`].
pub fn scaled_interval(interval: [f32; 2], scale: f32) -> [f32; 2] {
    interval.map(|gap| (gap * scale).min(MAX_GAP_SECONDS))
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
    if !platform::available() {
        return false;
    }
    match chosen_sound(read, None) {
        Ok(_) => true,
        // A phone with a player that offers none: the page plays its own sound, which a call's session may not let out.
        Err(error) => {
            log::warn!(target: "cue", "cue: not offered, the bundled web's sounds are not usable: {error}");
            false
        }
    }
}

/// What a start plays: `sound` at `gain_db` and the person's frequency, as [`render`] makes it, keyed by the three so that a
/// start of what already plays is told from one of something else (another frequency while it plays plays the new one).
pub struct Prepared {
    pub key: String,
    pub wav: Vec<u8>,
}

/// A sound decoded: its samples and their rate.
type Decoded = std::sync::Arc<(Vec<f32>, u32)>;
/// The decoded sound last played, by id: decoding it again for every turn of the agent's work is not needed.
static DECODED: std::sync::Mutex<Option<(String, Decoded)>> = std::sync::Mutex::new(None);

/// Reads, decodes and renders the sound a start asks for (off the main thread: it decodes), its silences drawn at the person's
/// frequency (`interval_scale`, from [`interval_scale`]). `seed` draws them.
pub fn prepare_cue(
    read: impl Fn(&str) -> Option<Vec<u8>>,
    sound: Option<&str>,
    gain_db: f32,
    interval_scale: f32,
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
        key: format!("{}@{gain_db}x{interval_scale}", sound.id),
        wav: render(
            samples,
            rate,
            scaled_interval(sound.interval, interval_scale),
            gain_db,
            seed,
        )?,
    })
}

/// When a start plays: at once, or only while the screen is locked (the page's "only with the screen off", its start's
/// `whileLocked`). A phone's page cannot apply that one itself: with the screen locked its webview may not run, neither to ask
/// whether the screen is locked nor to start the cue then, and iOS says it only some seconds after the lock. So the page starts
/// the cue while the agent works, whatever the screen, and the app plays it while the screen is locked: from the lock to the
/// unlock, again at each lock, until the page's stop (docs/BRIDGE.md -> "The cues").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// From the start to the stop.
    Always,
    /// Only while the screen is locked, between the start and the stop.
    WhileLocked,
}

impl When {
    /// What a start asks for: `whileLocked` true is the lock's; anything else (none: an older page) plays at once.
    pub fn asked(while_locked: Option<bool>) -> When {
        if while_locked == Some(true) {
            When::WhileLocked
        } else {
            When::Always
        }
    }
}

/// A start that plays only while the screen is locked, held between the page's start and its stop.
struct Held {
    cue: Cue,
    prepared: Prepared,
    ticket: u64,
}

/// What the lock starts and stops: the phone's player ([`System`]), or a test's.
trait Player {
    /// Plays the cue's loop (the same `key` again changes nothing; another replaces it).
    fn start(&mut self, cue: Cue, key: &str, wav: &[u8]) -> Result<(), String>;
    /// Stops it; nothing when it does not play.
    fn stop(&mut self, cue: Cue);
}

/// The phone's own player (`platform::cue`), on the main thread.
struct System;

impl Player for System {
    fn start(&mut self, cue: Cue, key: &str, wav: &[u8]) -> Result<(), String> {
        platform::start(cue, key, wav)
    }

    fn stop(&mut self, cue: Cue) {
        platform::stop(cue);
    }
}

/// Whether the screen is locked, as the system last said it (`platform::screen::watch`), and the cue held for the lock: what
/// the page's starts and stops and the system's locks and unlocks do to the player.
struct Lock {
    locked: bool,
    held: Option<Held>,
}

impl Lock {
    /// A start that is current: [`When::Always`] plays at once (and nothing is held for the lock any more);
    /// [`When::WhileLocked`] is held, and plays now if the screen is locked, else what played stops.
    fn start(
        &mut self,
        cue: Cue,
        prepared: Prepared,
        ticket: u64,
        when: When,
        player: &mut impl Player,
    ) -> Result<(), String> {
        match when {
            When::Always => {
                self.held = None;
                player.start(cue, &prepared.key, &prepared.wav)
            }
            When::WhileLocked => {
                let held = self.held.insert(Held {
                    cue,
                    prepared,
                    ticket,
                });
                if self.locked {
                    player.start(cue, &held.prepared.key, &held.prepared.wav)
                } else {
                    player.stop(cue);
                    Ok(())
                }
            }
        }
    }

    /// The page's stop or a call's end: the player stops, and nothing is held for the lock any more.
    fn stop(&mut self, cue: Cue, player: &mut impl Player) {
        self.held = None;
        player.stop(cue);
    }

    /// The system says whether the screen is locked: the held cue plays from a lock and stops at an unlock, unless it is stale
    /// (`current` says whether a ticket is: anything asked for after its start makes it so, and its stop is on its way).
    /// Answers whether the player was asked anything.
    fn changed(
        &mut self,
        locked: bool,
        current: impl Fn(u64) -> bool,
        player: &mut impl Player,
    ) -> Result<bool, String> {
        let was = std::mem::replace(&mut self.locked, locked);
        let Some(held) = self
            .held
            .as_ref()
            .filter(|held| current(held.ticket) && locked != was)
        else {
            return Ok(false);
        };
        if locked {
            player.start(held.cue, &held.prepared.key, &held.prepared.wav)?;
        } else {
            player.stop(held.cue);
        }
        Ok(true)
    }
}

thread_local! {
    /// On the main thread, where the player is and where the system tells the lock.
    static LOCK: std::cell::RefCell<Lock> = const { std::cell::RefCell::new(Lock { locked: false, held: None }) };
}

/// Plays `cue` until [`stop_cue`] (`when` [`When::WhileLocked`]: only while the screen is locked, from now on): a start of what
/// already plays (the same `key`) changes nothing, of something else replaces it. On the main thread. Nothing if `ticket` (from
/// [`cue_ticket`], when the start was asked for) is stale: the page stopped it, asked for another, or the call ended in between.
/// A stop or an end asked for once this check has passed moves the ticket on and then queues its [`stop_cue`] on this same
/// thread, behind this start: what it plays is stopped right after.
pub fn start_cue(cue: Cue, prepared: Prepared, ticket: u64, when: When) -> Result<(), String> {
    if !cue_current(ticket) {
        log::info!(target: "cue", "cue {cue:?}: a start superseded before it played");
        return Ok(());
    }
    LOCK.with_borrow_mut(|lock| {
        if when == When::WhileLocked {
            log::info!(
                target: "cue",
                "cue {cue:?}: plays while the screen is locked (locked now: {})",
                lock.locked
            );
        }
        let started = lock.start(cue, prepared, ticket, when, &mut System);
        if started.is_ok() && (when == When::Always || lock.locked) {
            log::info!(target: "cue", "cue {cue:?}: plays");
        }
        started
    })
}

/// Stops it, and what was held for the lock; nothing when it does not play. On the main thread.
pub fn stop_cue(cue: Cue) {
    LOCK.with_borrow_mut(|lock| lock.stop(cue, &mut System));
}

/// The system says the screen locked or unlocked, on the main thread: the cue held for the lock starts or stops.
fn screen_changed(locked: bool) {
    LOCK.with_borrow_mut(|lock| {
        if lock.locked != locked {
            log::info!(target: "cue", "screen: {}", if locked { "locked" } else { "unlocked" });
        }
        match lock.changed(locked, cue_current, &mut System) {
            Ok(true) => log::info!(
                target: "cue",
                "cue: {} with the screen",
                if locked { "plays" } else { "stops" }
            ),
            Ok(false) => {}
            Err(error) => log::warn!(target: "cue", "cue: did not start at the lock: {error}"),
        }
    });
}

/// From now on the system tells the cues each lock and unlock of the screen (where it tells them: iOS). Once, as the app starts.
pub fn follow_the_lock<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    crate::platform::screen::watch(app, screen_changed);
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
        // Past it, the start is a no-op (it never reaches the player, nor is it held for the lock), whatever the OS.
        assert_eq!(
            start_cue(Cue::Thinking, prepared, asked, When::WhileLocked),
            Ok(())
        );
        assert!(LOCK.with_borrow(|lock| lock.held.is_none()));
        // A later start wins over an earlier one still being read; the page's stop (an end) over both.
        let first = cue_ticket();
        let second = cue_ticket();
        assert!(!cue_current(first) && cue_current(second));
        end_cues();
        assert!(!cue_current(second));
    }

    /// A player that writes down what the lock asks of it.
    #[derive(Default)]
    struct Recorder(Vec<String>);

    impl Player for Recorder {
        fn start(&mut self, _: Cue, key: &str, _: &[u8]) -> Result<(), String> {
            self.0.push(format!("start {key}"));
            Ok(())
        }

        fn stop(&mut self, _: Cue) {
            self.0.push("stop".into());
        }
    }

    impl Recorder {
        fn took(&mut self) -> Vec<String> {
            std::mem::take(&mut self.0)
        }
    }

    const KEY: &str = "soft-heartbeat@0x1";

    fn prepared() -> Prepared {
        Prepared {
            key: KEY.into(),
            wav: vec![1, 2, 3],
        }
    }

    fn unlocked() -> Lock {
        Lock {
            locked: false,
            held: None,
        }
    }

    const PLAYS: &str = "start soft-heartbeat@0x1";

    #[test]
    fn only_a_start_asked_while_locked_waits_for_the_lock() {
        assert_eq!(When::asked(Some(true)), When::WhileLocked);
        // An older page's start, and the page's "always": at once.
        assert_eq!(When::asked(None), When::Always);
        assert_eq!(When::asked(Some(false)), When::Always);
    }

    #[test]
    fn a_cue_for_the_lock_plays_from_each_lock_to_the_unlock_until_the_page_stops_it() {
        let (mut lock, mut player, current) = (unlocked(), Recorder::default(), |_| true);
        // The agent works with the app in front or behind, the screen on: nothing plays (what played stops).
        lock.start(Cue::Thinking, prepared(), 1, When::WhileLocked, &mut player)
            .unwrap();
        assert_eq!(player.took(), ["stop"]);
        // The phone locks: it plays; the system saying so again asks nothing.
        assert_eq!(lock.changed(true, current, &mut player), Ok(true));
        assert_eq!(player.took(), [PLAYS]);
        assert_eq!(lock.changed(true, current, &mut player), Ok(false));
        assert!(player.took().is_empty());
        // Unlocked: it stops, and plays again at the next lock.
        assert_eq!(lock.changed(false, current, &mut player), Ok(true));
        assert_eq!(player.took(), ["stop"]);
        assert_eq!(lock.changed(false, current, &mut player), Ok(false));
        lock.changed(true, current, &mut player).unwrap();
        assert_eq!(player.took(), [PLAYS]);
        // The page's stop (the agent's answer, the setting, the call's end): it stops, and no lock plays it any more.
        lock.stop(Cue::Thinking, &mut player);
        assert_eq!(player.took(), ["stop"]);
        lock.changed(false, current, &mut player).unwrap();
        lock.changed(true, current, &mut player).unwrap();
        assert!(player.took().is_empty());
    }

    #[test]
    fn a_cue_for_the_lock_started_with_the_screen_locked_plays_at_once() {
        let (mut lock, mut player) = (unlocked(), Recorder::default());
        assert_eq!(
            lock.changed(true, |_| true, &mut player),
            Ok(false),
            "nothing held"
        );
        lock.start(Cue::Thinking, prepared(), 1, When::WhileLocked, &mut player)
            .unwrap();
        assert_eq!(player.took(), [PLAYS]);
    }

    #[test]
    fn a_cue_that_plays_at_once_is_none_of_the_lock_s() {
        let (mut lock, mut player, current) = (unlocked(), Recorder::default(), |_| true);
        // "Always": at once, and neither a lock nor an unlock touches it.
        lock.start(Cue::Thinking, prepared(), 1, When::Always, &mut player)
            .unwrap();
        assert_eq!(player.took(), [PLAYS]);
        lock.changed(true, current, &mut player).unwrap();
        lock.changed(false, current, &mut player).unwrap();
        assert!(player.took().is_empty());
        // The setting turned to "only with the screen off" while it plays, the screen on: it stops until the lock.
        lock.start(Cue::Thinking, prepared(), 2, When::WhileLocked, &mut player)
            .unwrap();
        assert_eq!(player.took(), ["stop"]);
        // And back to "always" while locked: it plays, and the unlock leaves it playing.
        lock.changed(true, current, &mut player).unwrap();
        player.took();
        lock.start(Cue::Thinking, prepared(), 3, When::Always, &mut player)
            .unwrap();
        lock.changed(false, current, &mut player).unwrap();
        assert_eq!(player.took(), [PLAYS]);
    }

    #[test]
    fn a_held_cue_asked_for_no_more_plays_at_no_lock() {
        let (mut lock, mut player) = (unlocked(), Recorder::default());
        lock.start(Cue::Thinking, prepared(), 1, When::WhileLocked, &mut player)
            .unwrap();
        player.took();
        // A call's end, or the page's stop, moved the ticket on: its stop is on its way to the main thread.
        assert_eq!(
            lock.changed(true, |ticket| ticket != 1, &mut player),
            Ok(false)
        );
        assert!(player.took().is_empty());
    }

    #[test]
    fn the_frequency_multiplies_the_silence_after_each_play_and_none_is_the_sound_s_own() {
        // None, or what a page cannot mean (not a number, out of the page's range), is the sound's own cadence.
        assert_eq!(interval_scale(None), 1.0);
        assert_eq!(interval_scale(Some(f64::NAN)), 1.0);
        assert_eq!(interval_scale(Some(f64::INFINITY)), 1.0);
        assert_eq!(interval_scale(Some(1.5)), 1.5);
        assert_eq!(interval_scale(Some(0.5)), 0.5);
        assert_eq!(interval_scale(Some(3.0)), 3.0);
        assert_eq!(interval_scale(Some(0.1)), 1.0);
        assert_eq!(interval_scale(Some(9.0)), 1.0);
        assert_eq!(interval_scale(Some(-2.0)), 1.0);
        // The soft heartbeat's 7–10 s: half, as is, three times.
        assert_eq!(scaled_interval([7.0, 10.0], 0.5), [3.5, 5.0]);
        assert_eq!(scaled_interval([7.0, 10.0], 1.0), [7.0, 10.0]);
        assert_eq!(scaled_interval([7.0, 10.0], 3.0), [21.0, 30.0]);
        // The web's longest gap (12 s) three times is what the app takes at most; a longer one is drawn at that.
        assert_eq!(scaled_interval([10.0, 12.0], 3.0), [30.0, 36.0]);
        assert_eq!(scaled_interval([20.0, 30.0], 3.0), [36.0, 36.0]);
    }

    #[test]
    fn another_frequency_is_another_loop_and_its_gaps_are_the_scaled_ones() {
        let rate = 8_000;
        let play = tone(rate, 0.5);
        let gaps = |wav: &[u8]| {
            // The silences between the plays, in seconds (a play is 0.5 s of tone, never a run of exact zeros).
            let all = samples(wav);
            let mut runs = Vec::new();
            let mut run = 0usize;
            for sample in all {
                if sample == 0 {
                    run += 1;
                } else {
                    if run > rate as usize / 10 {
                        runs.push(run as f32 / rate as f32);
                    }
                    run = 0;
                }
            }
            runs
        };
        let own = render(&play, rate, scaled_interval([2.0, 2.0], 1.0), 0.0, 3).unwrap();
        let slow = render(&play, rate, scaled_interval([2.0, 2.0], 3.0), 0.0, 3).unwrap();
        assert!(
            gaps(&own).iter().all(|gap| (gap - 2.0).abs() < 0.05),
            "{:?}",
            gaps(&own)
        );
        assert!(
            gaps(&slow).iter().all(|gap| (gap - 6.0).abs() < 0.05),
            "{:?}",
            gaps(&slow)
        );
        assert!(!gaps(&slow).is_empty());
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
