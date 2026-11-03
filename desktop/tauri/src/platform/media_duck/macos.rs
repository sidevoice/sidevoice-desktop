//! macOS has no public per-app volume or global pause. Lowering is therefore the default output device's own volume, so
//! the app's voice is lowered with the rest; pausing reaches Music and Spotify, asked through Apple events (the person
//! allows each once: Privacy & Security -> Automation), and never presses a media key, which toggles and can start
//! music. Both are described in docs/MEDIA_DUCK.md.
use super::{Lowering, Mode, Os, Pausing, Player};
use objc2_core_audio::{
    kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeOutput,
    kAudioObjectSystemObject, AudioObjectGetPropertyData, AudioObjectPropertyAddress, AudioObjectSetPropertyData,
};
use std::process::Command;
use std::ptr::NonNull;

pub(super) struct Native;

/// The key prefix of an output device's own volume, then its id: a volume is given back to the device it was taken from.
const SYSTEM: &str = "system:";
/// The key prefix of an app that takes Apple events, then its name.
const APP: &str = "app:";
/// The apps whose playback can be paused and resumed without a media key.
const APPS: [&str; 2] = ["Music", "Spotify"];
/// `kAudioHardwareServiceDeviceProperty_VirtualMainVolume`, the device's whole volume as the volume keys set it
/// (AudioToolbox's AudioHardwareService.h; no generated binding carries it).
const VIRTUAL_MAIN_VOLUME: u32 = 0x766d_7663;

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress { mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain }
}

/// The default output device, and its volume when it has one (some outputs, HDMI among them, have none).
fn output_volume() -> Option<(u32, f32)> {
    // SAFETY: Core Audio property reads into locals of the size each property is documented to have.
    unsafe {
        let mut device: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let mut at = address(kAudioHardwarePropertyDefaultOutputDevice, 0);
        let status = AudioObjectGetPropertyData(
            kAudioObjectSystemObject as u32,
            NonNull::from(&mut at),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut device).cast(),
        );
        if status != 0 || device == 0 {
            return None;
        }
        let mut volume: f32 = 0.0;
        let mut size = std::mem::size_of::<f32>() as u32;
        let mut at = address(VIRTUAL_MAIN_VOLUME, kAudioObjectPropertyScopeOutput);
        let status = AudioObjectGetPropertyData(
            device,
            NonNull::from(&mut at),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut volume).cast(),
        );
        (status == 0).then_some((device, volume))
    }
}

/// What the app named `app` says of its playback, `None` when it is not open or the person has not allowed it.
fn state(app: &str) -> Option<String> {
    // `is running` first: telling an app that is closed would open it.
    let script = format!(
        "if application \"{app}\" is running then tell application \"{app}\" to return (player state as string)"
    );
    let out = Command::new("/usr/bin/osascript").args(["-e", &script]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn tell(app: &str, command: &str) -> bool {
    // Only an app that is open: the same guard as `state`.
    let script = format!("if application \"{app}\" is running then tell application \"{app}\" to {command}");
    Command::new("/usr/bin/osascript").args(["-e", &script]).output().is_ok_and(|o| o.status.success())
}

impl Os for Native {
    const LOWERING: Lowering = Lowering::System;
    const PAUSING: Pausing = Pausing::SomeApps;

    fn players(mode: Mode) -> Vec<Player> {
        match mode {
            // Whatever plays, the system volume is all there is: it counts as playing.
            Mode::Lower => output_volume()
                .map(|(device, volume)| Player {
                    key: format!("{SYSTEM}{device}"),
                    playing: true,
                    volume: Some(volume),
                    pausable: false,
                })
                .into_iter()
                .collect(),
            Mode::Pause => APPS
                .iter()
                .filter_map(|app| {
                    let state = state(app)?;
                    Some(Player {
                        key: format!("{APP}{app}"),
                        playing: state == "playing",
                        volume: None,
                        pausable: true,
                    })
                })
                .collect(),
            Mode::Off => Vec::new(),
        }
    }

    fn set_volume(key: &str, volume: f32) -> bool {
        let Some(device) = key.strip_prefix(SYSTEM).and_then(|id| id.parse::<u32>().ok()) else { return false };
        let mut volume = volume;
        let mut at = address(VIRTUAL_MAIN_VOLUME, kAudioObjectPropertyScopeOutput);
        // SAFETY: Core Audio property write of one f32, the size this property has.
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
