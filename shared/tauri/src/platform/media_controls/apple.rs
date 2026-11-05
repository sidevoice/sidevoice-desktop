//! Headset buttons and Now Playing during a call, the same MediaPlayer and AVFAudio code on macOS and iOS
//! (docs/MACOS.md → "Headset buttons", docs/MEDIA_CONTROLS.md). `macos.rs` and `ios.rs` are the two OS bodies over it.
//!
//! - **Primary, any headset** (Bluetooth or wired): the play/pause button reaches the system as a media command
//!   (AVRCP for Bluetooth), which it gives to the "Now Playing" app. During a call this app is that app
//!   (`MPNowPlayingInfoCenter`, with `playbackState` set as Apple requires on macOS) and maps the commands to the
//!   microphone: play → unmute, pause → toggle (a click may arrive as pause while muted), toggle → toggle. On iOS the
//!   same commands come from the lock screen and Control Center, where the call is shown, and the app is the Now Playing
//!   app while its audio session is active (the mobile app's host activates it).
//! - **Extra, AirPods / Beats** (macOS 14+, iOS 17+): the stem-press mute gesture arrives through
//!   `AVAudioApplication.setInputMuteStateChangeHandler`; the app keeps that state in step with its own mute.
//! - No CallKit: neither needs it.
//!
//! Every event that arrives is recorded in the app log (`headset …` lines), so which buttons of a headset reach the
//! app can be read there.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_avf_audio::AVAudioApplication;
use objc2_foundation::{NSDictionary, NSString};
use objc2_media_player::{
    MPMediaItemPropertyArtist, MPMediaItemPropertyTitle, MPNowPlayingInfoCenter, MPNowPlayingPlaybackState,
    MPRemoteCommand, MPRemoteCommandCenter, MPRemoteCommandEvent, MPRemoteCommandHandlerStatus,
};
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use crate::headset::{media_command_action, mute_gesture_action, Changed};
use crate::platform::media_controls::{CallState, CommandSink, MediaCommand};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

/// What differs between macOS and iOS in this code, one method per difference, each with what an OS that does not have it
/// does by default; `macos.rs` and `ios.rs` implement it for their `Native`.
pub(super) trait Os {
    /// Says whether the app plays (`MPNowPlayingInfoCenter.playbackState`). Only macOS reads it, and requires it whenever
    /// playback starts or stops; iOS takes it from the app's audio session.
    fn set_playback_state(_center: &MPNowPlayingInfoCenter, _state: MPNowPlayingPlaybackState) {}

    /// Starts hearing the AirPods / Beats mute gesture, each one handed to [`heard`]; whether it does. Called on the main
    /// thread, with `AVAudioApplication` present. None by default.
    fn listen_for_mute_gesture(_app: &AppHandle) -> bool {
        false
    }

    /// Stops hearing it.
    fn stop_listening_for_mute_gesture() {}

    /// Tells the system the app no longer takes remote-control events (iOS's `UIApplication`), so that it stops being the
    /// Now Playing app. Main thread. Nothing by default.
    fn end_remote_control_events() {}
}

struct Headset {
    call: Mutex<CallState>,
    sink: CommandSink,
}

impl Headset {
    /// One event that arrived and what the app did with it, for the app log.
    fn record(&self, source: &str, action: &str) {
        sidevoice_shared::debug::debug(&format!("headset {source} → {action}"));
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
            (headset.sink)(MediaCommand::ToggleMute);
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
            let _ = app.clone().run_on_main_thread(move || (app.state::<Headset>().sink)(MediaCommand::ToggleMute));
            headset.record(source, what);
        }
        None => headset.record(source, "ya estaba así"),
    }
    true
}

/// The mute gesture arrived (as the system says it, on any thread): what the call does with it, unless it is the app's
/// own change coming back. Returns whether the call's microphone is now as asked: false when there is no call to mute.
pub fn heard(app: &AppHandle, muted: bool) -> bool {
    if echo(muted) {
        return true; // the app's own change, already done
    }
    on_mute_gesture(app, muted)
}

/// Keeps where the commands go, once; their handlers are registered while a call is shown ([`REGISTERED`]).
pub fn attach<N: Os>(app: &AppHandle, sink: CommandSink) {
    app.manage(Headset { call: Mutex::default(), sink });
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || setup::<N>(&app2));
}

/// Follows the call: the commands enabled and the call shown while there is one, on the main thread.
pub fn update<N: Os>(app: &AppHandle, snapshot: &CallState) {
    let Some(headset) = app.try_state::<Headset>() else { return };
    *headset.call.lock().unwrap() = snapshot.clone();
    let active = headset.active();
    let now_playing = active && !snapshot.now_playing_off;
    let title = snapshot.title.trim().to_owned();
    let (mic, in_call) = (snapshot.mic_enabled, snapshot.ready && snapshot.joined);
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || apply::<N>(&app2, active, now_playing, in_call, mic, &title));
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

/// Every command of the shared command center, by name: the app registers only [`commands`], but any of them with a
/// target, or enabled, keeps the app the Now Playing app as much (HANDOVER-live-activity R2-6), so all of them are let go.
fn every_command() -> Vec<(Retained<MPRemoteCommand>, &'static str)> {
    // SAFETY: as in `commands`; each subclass is a command.
    let center = unsafe { MPRemoteCommandCenter::sharedCommandCenter() };
    unsafe {
        vec![
            (center.playCommand(), "play"),
            (center.pauseCommand(), "pause"),
            (center.togglePlayPauseCommand(), "toggle"),
            (center.stopCommand(), "stop"),
            (center.nextTrackCommand(), "next"),
            (center.previousTrackCommand(), "previous"),
            (Retained::into_super(center.skipForwardCommand()), "skipForward"),
            (Retained::into_super(center.skipBackwardCommand()), "skipBackward"),
            (center.seekForwardCommand(), "seekForward"),
            (center.seekBackwardCommand(), "seekBackward"),
            (Retained::into_super(center.changePlaybackPositionCommand()), "changePlaybackPosition"),
            (Retained::into_super(center.changePlaybackRateCommand()), "changePlaybackRate"),
            (Retained::into_super(center.changeRepeatModeCommand()), "changeRepeatMode"),
            (Retained::into_super(center.changeShuffleModeCommand()), "changeShuffleMode"),
            (Retained::into_super(center.ratingCommand()), "rating"),
            (Retained::into_super(center.likeCommand()), "like"),
            (Retained::into_super(center.dislikeCommand()), "dislike"),
            (Retained::into_super(center.bookmarkCommand()), "bookmark"),
            (center.enableLanguageOptionCommand(), "enableLanguageOption"),
            (center.disableLanguageOptionCommand(), "disableLanguageOption"),
        ]
    }
}

/// Whether the command handlers are registered now: only while Now Playing shows the call. A handler registered is what
/// makes the system take the app for the Now Playing app (iOS keeps an empty card on the Lock Screen for it, even with no
/// Now Playing info and every command disabled), so none is registered at launch, and they go when Now Playing does.
/// Only touched on the main thread.
static REGISTERED: Mutex<bool> = Mutex::new(false);

/// What the last [`apply`] had: whether a call was active, and whether Now Playing showed it. None before the first.
static LAST: Mutex<Option<(bool, bool)>> = Mutex::new(None);

/// At launch: no handlers yet (see [`REGISTERED`]); they are registered when a call is shown.
fn setup<N: Os>(_app: &AppHandle) {
    release::<N>();
}

/// What Now Playing holds now, for the app log (`call: now playing …`): `released`, whether the app's side holds nothing
/// of it (no command enabled, no handler, no info: what a [`release`] that worked leaves, the one at launch included),
/// then the commands enabled (MediaPlayer says which are, not which have targets: the app's own handlers are
/// `handlers`), and whether there is Now Playing info. A command nothing has touched reads as enabled. Whether iOS has
/// really stopped taking the app for its Now Playing app cannot be read in the process: only mediaremoted's log says.
fn census(when: &str) {
    // SAFETY: main thread; a plain property read.
    let enabled: Vec<&str> =
        every_command().iter().filter(|(command, _)| unsafe { command.isEnabled() }).map(|(_, name)| *name).collect();
    // SAFETY: as above.
    let info = unsafe { MPNowPlayingInfoCenter::defaultCenter().nowPlayingInfo() }.is_some();
    let handlers = *REGISTERED.lock().unwrap();
    let released = enabled.is_empty() && !handlers && !info;
    log::info!(
        "call: now playing {when}: released={released} enabled=[{}] handlers={handlers} info={info}",
        enabled.join(",")
    );
}

/// Registers the handlers of the commands, enabled, once.
fn register(app: &AppHandle) {
    for (command, name) in commands() {
        let app = app.clone();
        let handler = RcBlock::new(move |_event: NonNull<MPRemoteCommandEvent>| {
            on_media_command(&app, name);
            MPRemoteCommandHandlerStatus::Success
        });
        // SAFETY: the target the command center returns is kept by it; the block is retained by it too.
        unsafe {
            let _target: Retained<AnyObject> = command.addTargetWithHandler(&handler);
        }
    }
}

/// Lets go of Now Playing (HANDOVER-live-activity R2-6): every handler of every command removed and every command
/// disabled, no Now Playing info, and no more remote-control events, so that the app stops being the Now Playing app.
fn release<N: Os>() {
    for (command, _) in every_command() {
        // SAFETY: main thread; `nil` removes every target of the command.
        unsafe {
            command.removeTarget(None);
            command.setEnabled(false);
        }
    }
    // SAFETY: main thread; plain MediaPlayer calls.
    unsafe {
        let center = MPNowPlayingInfoCenter::defaultCenter();
        N::set_playback_state(&center, MPNowPlayingPlaybackState::Stopped);
        center.setNowPlayingInfo(None);
    }
    *REGISTERED.lock().unwrap() = false;
    N::end_remote_control_events();
}

fn apply<N: Os>(app: &AppHandle, active: bool, now_playing: bool, in_call: bool, mic_enabled: bool, title: &str) {
    // Let go of Now Playing whenever it is not to show the call and something changed: a call starting (the report that
    // starts the call's service), Now Playing giving way (the Live Activity runs), and a call ending. What was there at a
    // call's start, and what is left after, go to the app log.
    let last = LAST.lock().unwrap().replace((active, now_playing));
    let was_active = matches!(last, Some((true, _)));
    if !now_playing && last != Some((active, now_playing)) {
        if active && !was_active {
            census("at the call's start");
        }
        release::<N>();
        if active {
            census("let go");
        } else if was_active {
            census("after the call");
        }
    }
    if now_playing && !*REGISTERED.lock().unwrap() {
        register(app);
        *REGISTERED.lock().unwrap() = true;
        for (command, _) in commands() {
            // SAFETY: main thread; plain property set.
            unsafe { command.setEnabled(true) };
        }
    }
    // SAFETY: main thread (update() dispatches here); plain Foundation / MediaPlayer calls.
    unsafe {
        let center = MPNowPlayingInfoCenter::defaultCenter();
        if now_playing {
            // The app's name, and the conversation the call is on when there is one (no text of the app's own: a
            // conversation's title is the person's).
            let name = NSString::from_str("Sidevoice");
            let artist = NSString::from_str(title);
            let info = if title.is_empty() {
                let keys: [&NSString; 1] = [MPMediaItemPropertyTitle];
                let values: [&AnyObject; 1] = [&name];
                NSDictionary::from_slices(&keys, &values)
            } else {
                let keys: [&NSString; 2] = [MPMediaItemPropertyTitle, MPMediaItemPropertyArtist];
                let values: [&AnyObject; 2] = [&name, &artist];
                NSDictionary::from_slices(&keys, &values)
            };
            center.setNowPlayingInfo(Some(&info));
            // Apple: on macOS this must be set whenever playback starts or stops, or remote commands may not
            // arrive. "Playing" while the microphone is open, "paused" while muted (as the web interface does).
            N::set_playback_state(
                &center,
                if mic_enabled || !in_call {
                    MPNowPlayingPlaybackState::Playing
                } else {
                    MPNowPlayingPlaybackState::Paused
                },
            );
        }
    }
    gesture::<N>(app, active, in_call, mic_enabled);
    static SEEN: Changed = Changed(Mutex::new(None));
    let line = format!(
        "headset apply active={active} now_playing={now_playing} in_call={in_call} mic={mic_enabled} handlers={} gesture_api={} gesture_on={}",
        *REGISTERED.lock().unwrap(),
        gesture_api(),
        *GESTURE_ON.lock().unwrap()
    );
    if SEEN.is_new(&line) {
        sidevoice_shared::debug::debug(&line);
    }
}

/// AirPods / Beats mute gesture (macOS 14+, iOS 17+, where `AVAudioApplication` exists): heard while active, and the
/// system's input mute kept in step with the microphone.
fn gesture<N: Os>(app: &AppHandle, active: bool, in_call: bool, mic_enabled: bool) {
    if !gesture_api() {
        return;
    }
    // SAFETY: the class exists (checked above); main thread.
    unsafe {
        let shared = AVAudioApplication::sharedInstance();
        let mut on = GESTURE_ON.lock().unwrap();
        if active && !*on {
            *on = N::listen_for_mute_gesture(app);
        } else if !active && *on {
            N::stop_listening_for_mute_gesture();
            set_muted(&shared, false);
            *on = false;
        }
        if *on && in_call && shared.isInputMuted() == mic_enabled {
            // Keep the system's (and the AirPods') idea of "muted" in step with the call's.
            set_muted(&shared, !mic_enabled);
        }
    }
}
