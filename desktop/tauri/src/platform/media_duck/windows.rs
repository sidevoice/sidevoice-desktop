//! Windows: pause and resume through the system media transport controls (the apps that show in the volume flyout's media
//! card: browsers, Spotify, the Media Player), lower through each app's audio session (WASAPI) on the default output
//! device. This app's own sound and media session (its webview's helpers included) are never touched.
use super::{Lowering, Mode, Os, Pausing};
#[path = "ours.rs"]
mod ours;
use ours::Ours;
use sidevoice_desktop_core::media_duck::{Player, State};
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
    // SAFETY: the audio API hands over a NUL-terminated string it allocated; it is read once and freed.
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as _));
        s
    }
}

/// An audio session of another app: its instance id (what tells two sessions of one app apart, for as long as it lives),
/// its volume control, and whether it is making sound.
struct Audio {
    id: String,
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
            let active = control.GetState().is_ok_and(|s| s == AudioSessionStateActive);
            found.push(Audio { id: text(id), volume, active });
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
        state: if a.active { State::Playing } else { State::Stopped },
        volume,
        pausable: false,
    }
}

/// The audio session `key` names, on the device it names.
fn audio_by_key(key: &str, ours: &Ours) -> Option<(String, Audio)> {
    let (endpoint, id) = key.strip_prefix(AUDIO)?.split_once('|')?;
    // SAFETY: the endpoint id comes back from the system as it was given.
    let device = unsafe { enumerator()?.GetDevice(&HSTRING::from(endpoint)).ok()? };
    let found = audio_on(&device, ours)?.into_iter().find(|a| a.id == id)?;
    Some((endpoint.to_owned(), found))
}

/// Whether a media session is probably this app's own page (WebView2 offers the page's Media Session to the system):
/// it names this executable or the webview's.
fn is_ours(aumid: &str) -> bool {
    let aumid = aumid.to_lowercase();
    let exe = std::env::current_exe().ok().and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()));
    aumid.contains("msedgewebview2") || exe.is_some_and(|exe| !exe.is_empty() && aumid.contains(&exe))
}

/// Every media session, with the app it belongs to.
fn sessions() -> Vec<(String, Session)> {
    let Some(manager) = Manager::RequestAsync().ok().and_then(|op| op.join().ok()) else { return Vec::new() };
    let Ok(all) = manager.GetSessions() else { return Vec::new() };
    all.into_iter()
        .filter_map(|s| s.SourceAppUserModelId().ok().map(|id| (id.to_string(), s)))
        .filter(|(id, _)| !is_ours(id))
        .collect()
}

fn status(session: &Session) -> State {
    match session.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()) {
        Ok(Status::Playing) => State::Playing,
        Ok(Status::Paused) => State::Paused,
        _ => State::Stopped,
    }
}

/// One app's media: playing if any of its sessions plays, paused if one is paused and none plays.
fn media_player(aumid: &str, all: &[(String, Session)]) -> Option<Player> {
    let mine: Vec<&Session> = all.iter().filter(|(id, _)| id == aumid).map(|(_, s)| s).collect();
    if mine.is_empty() {
        return None;
    }
    let states: Vec<State> = mine.iter().map(|s| status(s)).collect();
    let state = if states.contains(&State::Playing) {
        State::Playing
    } else if states.contains(&State::Paused) {
        State::Paused
    } else {
        State::Stopped
    };
    let pausable = mine
        .iter()
        .any(|s| s.GetPlaybackInfo().and_then(|i| i.Controls()).and_then(|c| c.IsPauseEnabled()).unwrap_or(false));
    Some(Player { key: format!("{MEDIA}{aumid}"), state, volume: None, pausable })
}

impl Os for Native {
    const LOWERING: Lowering = Lowering::PerApp;
    const PAUSING: Pausing = Pausing::MediaControls;

    fn players(mode: Mode) -> Vec<Player> {
        com();
        match mode {
            Mode::Lower => {
                let ours = Ours::new();
                let Some(devices) = enumerator() else { return Vec::new() };
                // SAFETY: COM calls on interfaces created here.
                let Some(device) = (unsafe { devices.GetDefaultAudioEndpoint(eRender, eMultimedia) }).ok() else {
                    return Vec::new();
                };
                // SAFETY: the id is read once and freed.
                let endpoint = text(unsafe { device.GetId() }.unwrap_or_default());
                audio_on(&device, &ours).unwrap_or_default().iter().map(|a| audio_player(&endpoint, a)).collect()
            }
            Mode::Pause => {
                let all = sessions();
                let mut ids: Vec<&String> = all.iter().map(|(id, _)| id).collect();
                ids.sort();
                ids.dedup();
                ids.into_iter().filter_map(|id| media_player(id, &all)).collect()
            }
            Mode::Off => Vec::new(),
        }
    }

    fn player(key: &str) -> Option<Player> {
        com();
        if let Some(id) = key.strip_prefix(MEDIA) {
            return media_player(id, &sessions());
        }
        let (endpoint, a) = audio_by_key(key, &Ours::new())?;
        Some(audio_player(&endpoint, &a))
    }

    fn set_volume(key: &str, volume: f32) -> bool {
        com();
        let Some((_, a)) = audio_by_key(key, &Ours::new()) else { return false };
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

/// Asks every session of the app that is in state `from` (a paused session is not paused again, a playing one not played).
fn transport(key: &str, from: State, ask: impl Fn(&Session) -> Option<bool>) -> bool {
    com();
    let Some(id) = key.strip_prefix(MEDIA) else { return false };
    let asked: Vec<bool> = sessions()
        .iter()
        .filter(|(found, s)| found == id && status(s) == from)
        .map(|(_, s)| ask(s) == Some(true))
        .collect();
    !asked.is_empty() && asked.iter().all(|ok| *ok)
}
