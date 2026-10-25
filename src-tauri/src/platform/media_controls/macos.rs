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
//! Every event that arrives is recorded in the app log (`headset …` lines), so which buttons of a headset reach the
//! app can be read there.

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
use std::time::{Duration, Instant};

use super::{CommandSink, MediaControls};
use sidevoice_desktop_core::bridge::CallSnapshot;
use sidevoice_desktop_core::headset::{media_command_action, mute_gesture_action};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

struct Headset {
    call: Mutex<CallSnapshot>,
    sink: CommandSink,
}

impl Headset {
    /// One event that arrived and what the app did with it, for the app log.
    fn record(&self, source: &str, action: &str) {
        crate::debug(&format!("headset {source} → {action}"));
    }

    fn active(&self) -> bool {
        let call = self.call.lock().unwrap();
        call.ready && call.joined
    }
}

/// A media command arrived (on the main thread).
fn on_media_command(app: &AppHandle, command: &str) {
    let headset = app.state::<Headset>();
    let snapshot = headset.call.lock().unwrap().clone();
    let in_call = snapshot.ready && snapshot.joined;
    let action = if in_call { media_command_action(command, snapshot.mic_enabled) } else { None };
    match action {
        Some(what) => {
            (headset.sink)(sidevoice_desktop_core::bridge::Command::ToggleMute);
            headset.record(&format!("remote:{command}"), what);
        }
        None if in_call => headset.record(&format!("remote:{command}"), "ya estaba así"),
        None => headset.record(&format!("remote:{command}"), "sin llamada: solo registrado"),
    }
}

/// The AirPods gesture arrived (the system calls it on its own queue). Returns whether the call's microphone is
/// now as asked (what the system wants the handler to answer): false when there is no call to mute.
fn on_mute_gesture(app: &AppHandle, muted: bool) -> bool {
    let headset = app.state::<Headset>();
    let snapshot = headset.call.lock().unwrap().clone();
    let source = if muted { "airpods:mute" } else { "airpods:unmute" };
    if !(snapshot.ready && snapshot.joined) {
        headset.record(source, "sin llamada: solo registrado");
        return false;
    }
    match mute_gesture_action(muted, snapshot.mic_enabled) {
        Some(what) => {
            let app = app.clone();
            let _ = app.clone().run_on_main_thread(move || {
                (app.state::<Headset>().sink)(sidevoice_desktop_core::bridge::Command::ToggleMute)
            });
            headset.record(source, what);
        }
        None => headset.record(source, "ya estaba así"),
    }
    true
}

pub struct NativeMediaControls;

impl MediaControls for NativeMediaControls {
    fn support() -> Option<&'static str> {
        Some("native")
    }

    fn attach(app: &AppHandle, sink: CommandSink) {
        app.manage(Headset { call: Mutex::default(), sink });
        let app2 = app.clone();
        let _ = app.run_on_main_thread(move || setup(&app2));
    }

    fn update(app: &AppHandle, snapshot: &CallSnapshot) {
        let Some(headset) = app.try_state::<Headset>() else { return };
        *headset.call.lock().unwrap() = snapshot.clone();
        let active = headset.active();
        let title = if snapshot.title.trim().is_empty() { "Llamada".to_string() } else { snapshot.title.clone() };
        let (mic, in_call) = (snapshot.mic_enabled, snapshot.ready && snapshot.joined);
        let app2 = app.clone();
        let _ = app.run_on_main_thread(move || apply(&app2, active, in_call, mic, &title));
    }
}

/// Whether the gesture handler is installed now (only during a call).
static GESTURE_ON: Mutex<bool> = Mutex::new(false);
/// The state the app itself last set with `setInputMuted`, and when: the system calls the handler for that
/// too, and such an echo is neither a gesture to record nor one to act on.
static SET_BY_APP: Mutex<Option<(bool, Instant)>> = Mutex::new(None);

fn echo(muted: bool) -> bool {
    let mut last = SET_BY_APP.lock().unwrap();
    match *last {
        Some((value, at)) if value == muted && at.elapsed() < Duration::from_secs(3) => {
            *last = None;
            true
        }
        _ => false,
    }
}

fn set_muted(shared: &AVAudioApplication, muted: bool) {
    *SET_BY_APP.lock().unwrap() = Some((muted, Instant::now()));
    // SAFETY: the class exists (the caller checked); plain property set.
    if unsafe { shared.setInputMuted_error(muted) }.is_err() {
        *SET_BY_APP.lock().unwrap() = None;
    }
}

fn commands() -> [(Retained<MPRemoteCommand>, &'static str); 3] {
    // SAFETY: the shared command center exists for the process's lifetime.
    let center = unsafe { MPRemoteCommandCenter::sharedCommandCenter() };
    unsafe {
        [(center.playCommand(), "play"), (center.pauseCommand(), "pause"), (center.togglePlayPauseCommand(), "toggle")]
    }
}

fn gesture_api() -> bool {
    AnyClass::get(c"AVAudioApplication").is_some()
}

fn setup(app: &AppHandle) {
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
}

fn apply(app: &AppHandle, active: bool, in_call: bool, mic_enabled: bool, title: &str) {
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
                if echo(muted.as_bool()) {
                    return Bool::YES; // the app's own change, already done
                }
                Bool::new(on_mute_gesture(&app, muted.as_bool()))
            });
            if shared.setInputMuteStateChangeHandler_error(Some(&handler)).is_ok() {
                *on = true;
            }
        } else if !active && *on {
            let _ = shared.setInputMuteStateChangeHandler_error(None);
            set_muted(&shared, false);
            *on = false;
        }
        if *on && in_call && shared.isInputMuted() == mic_enabled {
            // Keep the system's (and the AirPods') idea of "muted" in step with the call's.
            set_muted(&shared, !mic_enabled);
        }
    }
}
