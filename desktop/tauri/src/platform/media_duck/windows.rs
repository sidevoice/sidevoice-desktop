//! Windows: pause and resume through the system media transport controls (the apps that show in the volume flyout's media
//! card: browsers, Spotify, the Media Player), lower through each app's audio session (WASAPI) on the default output
//! device. The app's own process is never touched.
use super::{Lowering, Mode, Os, Pausing, Player};
use windows::core::{Interface, PWSTR};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
    ISimpleAudioVolume, MMDeviceEnumerator,
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

fn sessions() -> Option<Vec<Session>> {
    let manager = Manager::RequestAsync().ok()?.join().ok()?;
    Some(manager.GetSessions().ok()?.into_iter().collect())
}

/// Every audio session of another process on the default output device: its instance id and its volume control.
fn audio() -> Option<Vec<(String, ISimpleAudioVolume, bool)>> {
    // SAFETY: COM calls on interfaces this function created, on a thread that initialised COM.
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
        let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None).ok()?;
        let list = manager.GetSessionEnumerator().ok()?;
        let ours = std::process::id();
        let mut found = Vec::new();
        for i in 0..list.GetCount().ok()? {
            let Ok(control) = list.GetSession(i) else { continue };
            let Ok(info) = control.cast::<IAudioSessionControl2>() else { continue };
            // The system sounds and the app's own sound are not other apps' media.
            if info.IsSystemSoundsSession().0 == 0 /* S_OK; S_FALSE is another app */ || info.GetProcessId().ok() == Some(ours)
            {
                continue;
            }
            let (Ok(id), Ok(volume)) = (info.GetSessionInstanceIdentifier(), control.cast::<ISimpleAudioVolume>())
            else {
                continue;
            };
            let active = control.GetState().is_ok_and(|s| s == AudioSessionStateActive);
            found.push((text(id), volume, active));
        }
        Some(found)
    }
}

impl Os for Native {
    const LOWERING: Lowering = Lowering::PerApp;
    const PAUSING: Pausing = Pausing::MediaControls;

    fn players(_: Mode) -> Vec<Player> {
        com();
        let mut players = Vec::new();
        for (id, volume, active) in audio().unwrap_or_default() {
            // SAFETY: the volume control was obtained above and is read on this thread.
            let volume = unsafe { volume.GetMasterVolume() }.ok();
            players.push(Player { key: format!("{AUDIO}{id}"), playing: active, volume, pausable: false });
        }
        for session in sessions().unwrap_or_default() {
            let (Ok(id), Ok(info)) = (session.SourceAppUserModelId(), session.GetPlaybackInfo()) else { continue };
            players.push(Player {
                key: format!("{MEDIA}{id}"),
                playing: info.PlaybackStatus().is_ok_and(|s| s == Status::Playing),
                volume: None,
                pausable: info.Controls().and_then(|c| c.IsPauseEnabled()).unwrap_or(false),
            });
        }
        players
    }

    fn set_volume(key: &str, volume: f32) -> bool {
        com();
        let Some(id) = key.strip_prefix(AUDIO) else { return false };
        // SAFETY: the volume control belongs to this call's own enumeration.
        audio().unwrap_or_default().into_iter().any(|(found, control, _)| {
            found == id && unsafe { control.SetMasterVolume(volume, std::ptr::null()) }.is_ok()
        })
    }

    fn pause(key: &str) -> bool {
        transport(key, |s| s.TryPauseAsync().ok()?.join().ok())
    }

    fn play(key: &str) -> bool {
        transport(key, |s| s.TryPlayAsync().ok()?.join().ok())
    }
}

fn transport(key: &str, ask: impl Fn(&Session) -> Option<bool>) -> bool {
    com();
    let Some(id) = key.strip_prefix(MEDIA) else { return false };
    sessions()
        .unwrap_or_default()
        .iter()
        .find(|s| s.SourceAppUserModelId().is_ok_and(|found| found == id))
        .is_some_and(|s| ask(s) == Some(true))
}
