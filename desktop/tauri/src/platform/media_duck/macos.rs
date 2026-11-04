//! macOS has no public per-app volume or global pause. Lowering is therefore the default output device's own volume, so
//! the app's voice is lowered with the rest, and it is done only while Music or Spotify is playing (the only players
//! whose state can be asked without a private API); pausing reaches those two. Both ask them through Apple events (the
//! person allows each once: Privacy & Security -> Automation). No media key is ever sent: it toggles and can start music.
//! See docs/MEDIA_DUCK.md.
use super::{Lowering, Mode, Os, Pausing};
use objc2_core_audio::{
    kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeOutput,
    kAudioObjectSystemObject, AudioObjectGetPropertyData, AudioObjectPropertyAddress, AudioObjectSetPropertyData,
};
use objc2_core_services::{
    typeApplicationBundleID, typeWildCard, AECreateDesc, AEDesc, AEDeterminePermissionToAutomateTarget, AEDisposeDesc,
};
use sidevoice_desktop_core::media_duck::{Access, Player, Seen, State};
use std::process::Command;
use std::ptr::NonNull;

pub(super) struct Native;

/// The key prefix of an output device's own volume, then its id: a volume is given back to the device it was taken from.
const SYSTEM: &str = "system:";
/// The key prefix of an app that takes Apple events, then its name.
const APP: &str = "app:";
/// The apps whose playback can be asked and paused without a media key, with their bundle ids.
const APPS: [(&str, &str); 2] = [("Music", "com.apple.Music"), ("Spotify", "com.spotify.client")];
/// `kAudioHardwareServiceDeviceProperty_VirtualMainVolume`, the device's whole volume as the volume keys set it
/// (AudioToolbox's AudioHardwareService.h; no generated binding carries it).
const VIRTUAL_MAIN_VOLUME: u32 = 0x766d_7663;
/// `errAEEventNotPermitted`: the person refused.
const DENIED: i32 = -1743;
/// `procNotFound`: the app is not open.
const NOT_OPEN: i32 = -600;

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress { mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain }
}

/// A device's volume, when it has one (some outputs, HDMI among them, have none).
fn volume_of(device: u32) -> Option<f32> {
    let mut volume: f32 = 0.0;
    let mut size = std::mem::size_of::<f32>() as u32;
    let mut at = address(VIRTUAL_MAIN_VOLUME, kAudioObjectPropertyScopeOutput);
    // SAFETY: a Core Audio property read into a local of the size this property has.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            NonNull::from(&mut at),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut volume).cast(),
        )
    };
    (status == 0).then_some(volume)
}

/// The default output device.
fn default_output() -> Option<u32> {
    let mut device: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let mut at = address(kAudioHardwarePropertyDefaultOutputDevice, 0);
    // SAFETY: a Core Audio property read into a local of the size this property has.
    let status = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject as u32,
            NonNull::from(&mut at),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut device).cast(),
        )
    };
    (status == 0 && device != 0).then_some(device)
}

/// What the system says of this app sending Apple events to `bundle`: 0 yes, `DENIED`, or another code (-1744: not asked
/// yet, -600: the app is not open). Asks the person only when `ask` is set.
fn permission(bundle: &str, ask: bool) -> i32 {
    let mut target = AEDesc { descriptorType: 0, dataHandle: std::ptr::null_mut() };
    // SAFETY: an address descriptor made from the bundle id's bytes, used for one call and disposed.
    unsafe {
        if AECreateDesc(typeApplicationBundleID, bundle.as_ptr().cast(), bundle.len() as _, &mut target) != 0 {
            return -1;
        }
        let status = AEDeterminePermissionToAutomateTarget(&target, typeWildCard, typeWildCard, ask);
        AEDisposeDesc(&mut target);
        status
    }
}

/// What the app named `app` says of its playback: `Gone` when it is not open, `Unknown` when the person has not allowed it
/// or it does not answer (a script that asked anyway would put a dialog in the middle of a call).
fn state(app: &str, bundle: &str) -> Result<State, Seen> {
    match permission(bundle, false) {
        0 => {}
        NOT_OPEN => return Err(Seen::Gone),
        _ => return Err(Seen::Unknown),
    }
    // `is running` first: telling an app that is closed would open it.
    let script = format!(
        "if application \"{app}\" is running then tell application \"{app}\" to return (player state as string)"
    );
    let Some(out) =
        Command::new("/usr/bin/osascript").args(["-e", &script]).output().ok().filter(|o| o.status.success())
    else {
        return Err(Seen::Unknown);
    };
    match String::from_utf8_lossy(&out.stdout).trim() {
        "playing" => Ok(State::Playing),
        "paused" => Ok(State::Paused),
        "" => Err(Seen::Gone),
        _ => Ok(State::Stopped),
    }
}

fn tell(app: &str, command: &str) -> bool {
    // Only an app that is open: the same guard as `state`.
    let script = format!("if application \"{app}\" is running then tell application \"{app}\" to {command}");
    Command::new("/usr/bin/osascript").args(["-e", &script]).output().is_ok_and(|o| o.status.success())
}

fn app_player(app: &str, bundle: &str) -> Result<Player, Seen> {
    Ok(Player {
        key: format!("{APP}{app}"),
        app: String::new(),
        state: state(app, bundle)?,
        volume: None,
        pausable: true,
    })
}

impl Os for Native {
    fn lowering() -> Lowering {
        Lowering::System
    }

    fn pausing() -> Pausing {
        Pausing::SomeApps
    }

    fn access() -> Access {
        let codes: Vec<i32> = APPS.iter().map(|(_, bundle)| permission(bundle, false)).collect();
        if codes.contains(&0) {
            Access::Granted
        } else if codes.contains(&DENIED) {
            Access::Denied
        } else {
            Access::Undetermined
        }
    }

    fn ask() {
        // The system asks about an app that is open; one that is closed cannot be asked.
        for (_, bundle) in APPS {
            permission(bundle, true);
        }
    }

    fn players(mode: Mode) -> Vec<Player> {
        match mode {
            // The system volume is lowered only while an app that can be asked is playing; for anything else playing
            // (a browser) nothing here can tell, and nothing is lowered.
            Mode::Lower => {
                let playing = APPS.iter().any(|(app, bundle)| state(app, bundle) == Ok(State::Playing));
                let Some(device) = default_output().filter(|_| playing) else { return Vec::new() };
                volume_of(device)
                    .map(|volume| Player {
                        key: format!("{SYSTEM}{device}"),
                        app: String::new(),
                        state: State::Playing,
                        volume: Some(volume),
                        pausable: false,
                    })
                    .into_iter()
                    .collect()
            }
            Mode::Pause => APPS.iter().filter_map(|(app, bundle)| app_player(app, bundle).ok()).collect(),
            Mode::Off => Vec::new(),
        }
    }

    fn player(key: &str, _app: &str) -> Seen {
        if let Some(device) = key.strip_prefix(SYSTEM).and_then(|id| id.parse::<u32>().ok()) {
            // That device, wherever the default has gone since.
            return volume_of(device).map_or(Seen::Gone, |volume| {
                Seen::Is(Player {
                    key: key.to_owned(),
                    app: String::new(),
                    state: State::Playing,
                    volume: Some(volume),
                    pausable: false,
                })
            });
        }
        let Some((name, bundle)) = key.strip_prefix(APP).and_then(|app| APPS.iter().find(|(name, _)| *name == app))
        else {
            return Seen::Gone;
        };
        app_player(name, bundle).map_or_else(|seen| seen, Seen::Is)
    }

    fn set_volume(key: &str, volume: f32) -> bool {
        let Some(device) = key.strip_prefix(SYSTEM).and_then(|id| id.parse::<u32>().ok()) else { return false };
        let mut volume = volume;
        let mut at = address(VIRTUAL_MAIN_VOLUME, kAudioObjectPropertyScopeOutput);
        // SAFETY: a Core Audio property write of one f32, the size this property has.
        unsafe {
            AudioObjectSetPropertyData(
                device,
                NonNull::from(&mut at),
                0,
                std::ptr::null(),
                std::mem::size_of::<f32>() as u32,
                NonNull::from(&mut volume).cast(),
            ) == 0
        }
    }

    fn pause(key: &str) -> bool {
        key.strip_prefix(APP).is_some_and(|app| tell(app, "pause"))
    }

    fn play(key: &str) -> bool {
        key.strip_prefix(APP).is_some_and(|app| tell(app, "play"))
    }
}
