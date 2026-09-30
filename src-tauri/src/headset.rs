//! Headset buttons during a call (docs/MACOS.md → "Headset buttons").
//!
//! - **Primary, any headset** (Bluetooth or wired): the play/pause button reaches macOS as a media command
//!   (AVRCP for Bluetooth); macOS gives it to the "Now Playing" app. During a call this app is that app
//!   (`MPNowPlayingInfoCenter`, with `playbackState` set as Apple requires on macOS) and maps the commands to the
//!   microphone: play → unmute, pause → toggle (a click may arrive as pause while muted), toggle → toggle.
//! - **Extra, AirPods / Beats** (macOS 14+): the stem-press mute gesture arrives through
//!   `AVAudioApplication.setInputMuteStateChangeHandler`; the app keeps that state in step with its own mute.
//! - No CallKit: neither needs it (Apple documents both for plain macOS apps).
//!
//! Every event that arrives is recorded (the settings window shows the last ones: "Botones del auricular"), so a
//! person can see which buttons of their own headset reach the app. `test()` makes the app the Now Playing app for
//! a minute without a call, to try buttons without calling anyone.

use serde::Serialize;
use sidevoice_desktop_core::bridge::CallSnapshot;
use sidevoice_desktop_core::headset::{media_command_action, mute_gesture_action};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

const KEEP: usize = 30;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ButtonEvent {
    /// Milliseconds since the Unix epoch.
    pub at: u128,
    /// `remote:play`, `remote:pause`, `remote:toggle`, `airpods:mute`, `airpods:unmute`, `system:…`.
    pub source: String,
    /// What the app did with it.
    pub action: String,
}

#[derive(Default)]
pub struct Headset {
    events: Mutex<VecDeque<ButtonEvent>>,
    call: Mutex<CallSnapshot>,
    test_until: Mutex<Option<Instant>>,
    /// Whether the AirPods gesture API exists on this Mac (macOS 14+).
    pub mute_gesture_api: Mutex<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    events: Vec<ButtonEvent>,
    in_call: bool,
    testing: bool,
    mute_gesture_api: bool,
    platform_supported: bool,
}

impl Headset {
    pub fn record(&self, source: &str, action: &str) {
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        crate::debug(&format!("headset {source} → {action}"));
        let mut events = self.events.lock().unwrap();
        events.push_front(ButtonEvent { at, source: source.into(), action: action.into() });
        events.truncate(KEEP);
    }

    fn testing(&self) -> bool {
        self.test_until.lock().unwrap().is_some_and(|until| Instant::now() < until)
    }

    fn active(&self) -> bool {
        let call = self.call.lock().unwrap();
        (call.ready && call.joined) || self.testing()
    }

    pub fn report(&self) -> Report {
        Report {
            events: self.events.lock().unwrap().iter().cloned().collect(),
            in_call: {
                let c = self.call.lock().unwrap();
                c.ready && c.joined
            },
            testing: self.testing(),
            mute_gesture_api: *self.mute_gesture_api.lock().unwrap(),
            platform_supported: cfg!(target_os = "macos"),
        }
    }
}

/// A media command arrived (on the main thread).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn on_media_command(app: &AppHandle, command: &str) {
    let headset = app.state::<Headset>();
    let snapshot = headset.call.lock().unwrap().clone();
    let in_call = snapshot.ready && snapshot.joined;
    let action = if in_call { media_command_action(command, snapshot.mic_enabled) } else { None };
    match action {
        Some(what) => {
            crate::send(app, sidevoice_desktop_core::bridge::Command::ToggleMute);
            headset.record(&format!("remote:{command}"), what);
        }
        None if in_call => headset.record(&format!("remote:{command}"), "ya estaba así"),
        None => headset.record(&format!("remote:{command}"), "sin llamada: solo registrado"),
    }
}

/// The AirPods gesture arrived (the system calls it on its own queue).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn on_mute_gesture(app: &AppHandle, muted: bool) {
    let headset = app.state::<Headset>();
    let snapshot = headset.call.lock().unwrap().clone();
    let source = if muted { "airpods:mute" } else { "airpods:unmute" };
    if !(snapshot.ready && snapshot.joined) {
        headset.record(source, "sin llamada: solo registrado");
        return;
    }
    match mute_gesture_action(muted, snapshot.mic_enabled) {
        Some(what) => {
            let app = app.clone();
            let _ = app
                .clone()
                .run_on_main_thread(move || crate::send(&app, sidevoice_desktop_core::bridge::Command::ToggleMute));
            headset.record(source, what);
        }
        None => headset.record(source, "ya estaba así"),
    }
}

/// Follows the call: Now Playing and the buttons while in a call (or testing), nothing otherwise.
pub fn update(app: &AppHandle, snapshot: &CallSnapshot) {
    let Some(headset) = app.try_state::<Headset>() else { return };
    *headset.call.lock().unwrap() = snapshot.clone();
    let active = headset.active();
    let title = if snapshot.title.trim().is_empty() { "Llamada".to_string() } else { snapshot.title.clone() };
    let (mic, in_call) = (snapshot.mic_enabled, snapshot.ready && snapshot.joined);
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || platform::apply(&app2, active, in_call, mic, &title));
}

/// Makes the app the Now Playing app for `seconds` without a call, to try a headset's buttons.
pub fn test(app: &AppHandle, seconds: u64) {
    let headset = app.state::<Headset>();
    *headset.test_until.lock().unwrap() = Some(Instant::now() + Duration::from_secs(seconds));
    headset.record("prueba", &format!("{seconds} s escuchando botones"));
    let snapshot = headset.call.lock().unwrap().clone();
    update(app, &snapshot);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(seconds) + Duration::from_millis(200));
        let snapshot = app.state::<Headset>().call.lock().unwrap().clone();
        update(&app, &snapshot);
    });
}

pub fn setup(app: &AppHandle) {
    app.manage(Headset::default());
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || platform::setup(&app2));
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{on_media_command, on_mute_gesture, Headset};
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2_avf_audio::AVAudioApplication;
    use objc2_foundation::{NSDictionary, NSString};
    use objc2_media_player::{
        MPMediaItemPropertyArtist, MPMediaItemPropertyTitle, MPNowPlayingInfoCenter, MPNowPlayingPlaybackState,
        MPRemoteCommand, MPRemoteCommandCenter, MPRemoteCommandEvent, MPRemoteCommandHandlerStatus,
    };
    use std::ptr::NonNull;
    use std::sync::Mutex;
    use tauri::{AppHandle, Manager};

    /// Whether the gesture handler is installed now (only during a call or a test).
    static GESTURE_ON: Mutex<bool> = Mutex::new(false);

    fn commands() -> [(Retained<MPRemoteCommand>, &'static str); 3] {
        // SAFETY: the shared command center exists for the process's lifetime.
        let center = unsafe { MPRemoteCommandCenter::sharedCommandCenter() };
        unsafe {
            [
                (center.playCommand(), "play"),
                (center.pauseCommand(), "pause"),
                (center.togglePlayPauseCommand(), "toggle"),
            ]
        }
    }

    fn gesture_api() -> bool {
        AnyClass::get(c"AVAudioApplication").is_some()
    }

    pub fn setup(app: &AppHandle) {
        for (command, name) in commands() {
            let app = app.clone();
            let handler = RcBlock::new(move |_event: NonNull<MPRemoteCommandEvent>| {
                on_media_command(&app, name);
                MPRemoteCommandHandlerStatus::Success
            });
            // SAFETY: the target the command center returns is kept by it; the block is retained by it too.
            unsafe {
                let _target: Retained<AnyObject> = command.addTargetWithHandler(&handler);
                command.setEnabled(false);
            }
        }
        *app.state::<Headset>().mute_gesture_api.lock().unwrap() = gesture_api();
    }

    pub fn apply(app: &AppHandle, active: bool, in_call: bool, mic_enabled: bool, title: &str) {
        // SAFETY: main thread (update() dispatches here); plain Foundation / MediaPlayer calls.
        unsafe {
            for (command, _) in commands() {
                command.setEnabled(active);
            }
            let center = MPNowPlayingInfoCenter::defaultCenter();
            if active {
                let keys: [&NSString; 2] = [MPMediaItemPropertyTitle, MPMediaItemPropertyArtist];
                let name = NSString::from_str("Sidevoice");
                let artist = NSString::from_str(title);
                let values: [&AnyObject; 2] = [&name, &artist];
                let info = NSDictionary::from_slices(&keys, &values);
                center.setNowPlayingInfo(Some(&info));
                // Apple: on macOS this must be set whenever playback starts or stops, or remote commands may not
                // arrive. "Playing" while the microphone is open, "paused" while muted (as the web interface does).
                center.setPlaybackState(if mic_enabled || !in_call {
                    MPNowPlayingPlaybackState::Playing
                } else {
                    MPNowPlayingPlaybackState::Paused
                });
            } else {
                center.setPlaybackState(MPNowPlayingPlaybackState::Stopped);
                center.setNowPlayingInfo(None);
            }
        }
        gesture(app, active, in_call, mic_enabled);
        crate::debug(&format!(
            "headset apply active={active} in_call={in_call} mic={mic_enabled} gesture_api={} gesture_on={}",
            gesture_api(),
            *GESTURE_ON.lock().unwrap()
        ));
    }

    /// AirPods / Beats mute gesture (macOS 14+): installed while active, kept in step with the microphone.
    fn gesture(app: &AppHandle, active: bool, in_call: bool, mic_enabled: bool) {
        if !gesture_api() {
            return;
        }
        // SAFETY: the class exists (checked above); the handler block is retained by the system.
        unsafe {
            let shared = AVAudioApplication::sharedInstance();
            let mut on = GESTURE_ON.lock().unwrap();
            if active && !*on {
                let app = app.clone();
                let handler = RcBlock::new(move |muted: Bool| -> Bool {
                    on_mute_gesture(&app, muted.as_bool());
                    Bool::YES
                });
                if shared.setInputMuteStateChangeHandler_error(Some(&handler)).is_ok() {
                    *on = true;
                }
            } else if !active && *on {
                let _ = shared.setInputMuted_error(false);
                let _ = shared.setInputMuteStateChangeHandler_error(None);
                *on = false;
            }
            if *on && in_call && shared.isInputMuted() == mic_enabled {
                // Keep the system's (and the AirPods') idea of "muted" in step with the call's.
                let _ = shared.setInputMuted_error(!mic_enabled);
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use tauri::AppHandle;

    pub fn setup(_app: &AppHandle) {}

    pub fn apply(_app: &AppHandle, _active: bool, _in_call: bool, _mic_enabled: bool, _title: &str) {}
}
