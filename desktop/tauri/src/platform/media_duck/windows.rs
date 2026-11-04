//! Windows: pause and resume through the system media transport controls (the apps that show in the volume flyout's media
//! card: browsers, Spotify, the Media Player), lower through each app's audio session (WASAPI) on the default output
//! device. This app's own sound and media session (its webview's helpers included) are never touched. The mixer keeps a
//! volume per app, so a lowered volume is given back to the app's next session too.
use super::{Lowering, Mode, Os, Pausing};
#[path = "ours.rs"]
mod ours;
use ours::Ours;
use sidevoice_desktop_core::media_duck::{Player, Seen, State};
use windows::core::{Interface, HSTRING, PWSTR};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2, IMMDevice,
    IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};

pub(super) struct Native;

const MEDIA: &str = "media:";
const AUDIO: &str = "audio:";

/// A worker thread's COM apartment; asking again on a thread that has one is harmless.
fn com() {
    // SAFETY: plain COM initialisation; a repeat or a different apartment already set on the thread is not an error here.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
}

fn text(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: the audio API hands over a NUL-terminated string it allocated; it is read once and freed.
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as _));
        s
    }
}

/// An audio session of another app: its instance id (what tells two sessions of one app apart, for as long as it lives),
/// its identifier (what the mixer keeps the app's volume under), its volume control, and whether it is making sound.
struct Audio {
    id: String,
    app: String,
    volume: ISimpleAudioVolume,
    active: bool,
}

/// The sessions of every process that is not this app's on `device`.
fn audio_on(device: &IMMDevice, ours: &Ours) -> Option<Vec<Audio>> {
    // SAFETY: COM calls on interfaces this function created, on a thread that initialised COM.
    unsafe {
        let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None).ok()?;
        let list = manager.GetSessionEnumerator().ok()?;
        let mut found = Vec::new();
        for i in 0..list.GetCount().ok()? {
            let Ok(control) = list.GetSession(i) else { continue };
            let Ok(info) = control.cast::<IAudioSessionControl2>() else { continue };
            // The system sounds are not another app's media (S_OK: they are; S_FALSE: another app); nor are this app's.
            if info.IsSystemSoundsSession().0 == 0 || info.GetProcessId().ok().is_some_and(|pid| ours.has(pid)) {
                continue;
            }
            let (Ok(id), Ok(volume)) = (info.GetSessionInstanceIdentifier(), control.cast::<ISimpleAudioVolume>())
            else {
                continue;
            };
            let app = info.GetSessionIdentifier().map(text).unwrap_or_default();
            let active = control.GetState().is_ok_and(|s| s == AudioSessionStateActive);
            found.push(Audio { id: text(id), app, volume, active });
        }
        Some(found)
    }
}

fn enumerator() -> Option<IMMDeviceEnumerator> {
    // SAFETY: creating the system's device enumerator on a thread that initialised COM.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok() }
}

/// An audio key is `audio:<endpoint id>|<session instance id>`: the device it was on, so it is found again there.
fn audio_key(endpoint: &str, id: &str) -> String {
    format!("{AUDIO}{endpoint}|{id}")
}

fn audio_player(endpoint: &str, a: &Audio) -> Player {
    // SAFETY: the volume control was obtained from this call's own enumeration.
    let volume = unsafe { a.volume.GetMasterVolume() }.ok();
    Player {
        key: audio_key(endpoint, &a.id),
        app: a.app.clone(),
        state: if a.active { State::Playing } else { State::Stopped },
        volume,
        pausable: false,
    }
}

/// The audio session `key` names, on the device it names; else, on that device, another session of `app`.
fn audio_by_key(key: &str, app: &str, ours: &Ours) -> Result<(String, Audio), Seen> {
    let Some((endpoint, id)) = key.strip_prefix(AUDIO).and_then(|k| k.split_once('|')) else { return Err(Seen::Gone) };
    let Some(devices) = enumerator() else { return Err(Seen::Unknown) };
    // SAFETY: the endpoint id comes back from the system as it was given.
    let Ok(device) = (unsafe { devices.GetDevice(&HSTRING::from(endpoint)) }) else { return Err(Seen::Gone) };
    let Some(all) = audio_on(&device, ours) else { return Err(Seen::Unknown) };
    let mut all = all.into_iter();
    let found = all.by_ref().find(|a| a.id == id).or_else(|| all.find(|a| !app.is_empty() && a.app == app));
    found.map(|a| (endpoint.to_owned(), a)).ok_or(Seen::Gone)
}

/// Whether a media session is probably this app's own page (WebView2 offers the page's Media Session to the system):
/// it names this executable or the webview's.
fn is_ours(aumid: &str) -> bool {
    let aumid = aumid.to_lowercase();
    let exe = std::env::current_exe().ok().and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()));
    aumid.contains("msedgewebview2") || exe.is_some_and(|exe| !exe.is_empty() && aumid.contains(&exe))
}

/// Every media session of another app, in the order the system lists them: an app's sessions are told apart by their
/// place among its own (`media:<app id>#<n>`).
fn sessions() -> Option<Vec<(String, Session)>> {
    let manager = Manager::RequestAsync().ok()?.join().ok()?;
    let all = manager.GetSessions().ok()?;
    let mut counts = std::collections::HashMap::<String, usize>::new();
    Some(
        all.into_iter()
            .filter_map(|s| s.SourceAppUserModelId().ok().map(|id| (id.to_string(), s)))
            .filter(|(id, _)| !is_ours(id))
            .map(|(id, s)| {
                let n = counts.entry(id.clone()).or_default();
                *n += 1;
                (format!("{MEDIA}{id}#{}", *n - 1), s)
            })
            .collect(),
    )
}

fn status(session: &Session) -> State {
    match session.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()) {
        Ok(Status::Playing) => State::Playing,
        Ok(Status::Paused) => State::Paused,
        _ => State::Stopped,
    }
}

fn media_player(key: &str, session: &Session) -> Player {
    let pausable =
        session.GetPlaybackInfo().and_then(|i| i.Controls()).and_then(|c| c.IsPauseEnabled()).unwrap_or(false);
    Player { key: key.to_owned(), app: String::new(), state: status(session), volume: None, pausable }
}

impl Os for Native {
    fn lowering() -> Lowering {
        Lowering::PerApp
    }

    fn pausing() -> Pausing {
        Pausing::MediaControls
    }

    fn players(mode: Mode) -> Vec<Player> {
        com();
        match mode {
            Mode::Lower => {
                let ours = Ours::new();
                let Some(devices) = enumerator() else { return Vec::new() };
                // SAFETY: COM calls on interfaces created here.
                let Ok(device) = (unsafe { devices.GetDefaultAudioEndpoint(eRender, eMultimedia) }) else {
                    return Vec::new();
                };
                // SAFETY: the id is read once and freed.
                let Ok(id) = (unsafe { device.GetId() }) else { return Vec::new() };
                let endpoint = text(id);
                if endpoint.is_empty() {
                    return Vec::new();
                }
                audio_on(&device, &ours).unwrap_or_default().iter().map(|a| audio_player(&endpoint, a)).collect()
            }
            Mode::Pause => {
                sessions().unwrap_or_default().iter().map(|(key, session)| media_player(key, session)).collect()
            }
            Mode::Off => Vec::new(),
        }
    }

    fn player(key: &str, app: &str) -> Seen {
        com();
        if key.starts_with(MEDIA) {
            return match sessions() {
                None => Seen::Unknown,
                Some(all) => {
                    all.iter().find(|(k, _)| k == key).map_or(Seen::Gone, |(k, s)| Seen::Is(media_player(k, s)))
                }
            };
        }
        match audio_by_key(key, app, &Ours::new()) {
            Ok((endpoint, a)) => Seen::Is(audio_player(&endpoint, &a)),
            Err(seen) => seen,
        }
    }

    fn set_volume(key: &str, volume: f32) -> bool {
        com();
        let Ok((_, a)) = audio_by_key(key, "", &Ours::new()) else { return false };
        // SAFETY: the volume control belongs to this call's own enumeration.
        unsafe { a.volume.SetMasterVolume(volume, std::ptr::null()) }.is_ok()
    }

    fn pause(key: &str) -> bool {
        transport(key, State::Playing, |s| s.TryPauseAsync().ok()?.join().ok())
    }

    fn play(key: &str) -> bool {
        transport(key, State::Paused, |s| s.TryPlayAsync().ok()?.join().ok())
    }
}

/// Asks the one session `key` names, if it is in state `from` (a paused session is not paused again).
fn transport(key: &str, from: State, ask: impl Fn(&Session) -> Option<bool>) -> bool {
    com();
    sessions()
        .unwrap_or_default()
        .iter()
        .find(|(k, s)| k == key && status(s) == from)
        .is_some_and(|(_, s)| ask(s) == Some(true))
}
