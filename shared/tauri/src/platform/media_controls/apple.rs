//! Headset buttons and Now Playing during a call, the same MediaPlayer and AVFAudio code on macOS and iOS
//! (docs/MACOS.md → "Headset buttons", docs/MEDIA_CONTROLS.md). `macos.rs` and `ios.rs` are the two OS bodies over it.
//!
//! - **Primary, any headset** (Bluetooth or wired), macOS only (`Os::NOW_PLAYING`): the play/pause button reaches the system
//!   as a media command (AVRCP for Bluetooth), which it gives to the "Now Playing" app. During a call this app is that app
//!   (`MPNowPlayingInfoCenter`, with `playbackState` set as Apple requires on macOS) and maps the commands to the
//!   microphone: play → unmute, pause → toggle (a click may arrive as pause while muted), toggle → toggle. Not on iOS, where a
//!   headset in a call is its hands-free and its presses are call control, which never reach the remote-command center
//!   (`ios.rs`, desktop#464).
//! - **Double press**, with Now Playing: a headset's or AirPods' double press of play/pause arrives as `nextTrackCommand`
//!   (`Os::NEXT_TRACK`), while the app shows the call as Now Playing. The app sends it as `MediaCommand::NextTrack`; a keyboard's
//!   next-track key arrives the same way.
//! - **AirPods / Beats** (macOS 14+, iOS 17+): the stem-press mute gesture ("Press to Mute and Unmute"), heard through
//!   `AVAudioApplication` (macOS: a handler, while a call is on; iOS: a notification, opted in at launch,
//!   `Os::MUTE_GESTURE_FOR_LIFE`); the app keeps that state in step with its own mute.
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

use crate::headset::{
    gesture_step, media_dispatch, mute_gesture_action, registered_commands, Changed, Dispatch, ALL_COMMANDS,
};
use crate::platform::media_controls::{CallState, CommandSink, MediaCommand};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

/// What differs between macOS and iOS in this code, one method per difference, each with what an OS that does not have it
/// does by default; `macos.rs` and `ios.rs` implement it for their `Native`.
pub(super) trait Os {
    /// Says whether the app plays (`MPNowPlayingInfoCenter.playbackState`). Only macOS reads it, and requires it whenever
    /// playback starts or stops.
    fn set_playback_state(_center: &MPNowPlayingInfoCenter, _state: MPNowPlayingPlaybackState) {}

    /// Whether the app shows a call as Now Playing, with the remote commands' handlers: where a headset's play/pause reaches the
    /// remote-command center during a call (macOS). False by default: nothing is registered, and everything is let go.
    const NOW_PLAYING: bool = false;

    /// Whether the next-track command is registered too, as a matter of course: a headset's or AirPods' double press of
    /// play/pause arrives as it (macOS). False by default.
    const NEXT_TRACK: bool = false;

    /// Whether the mute gesture is heard from launch for the process's life (iOS: observing the notification is the opt-in, done
    /// before any call's audio session is active), rather than only while a call is on. False by default.
    const MUTE_GESTURE_FOR_LIFE: bool = false;

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
    match media_dispatch(command, in_call, snapshot.mic_enabled) {
        Dispatch::NextTrack => {
            (headset.sink)(MediaCommand::NextTrack);
            headset.record("remote:next", "double press");
        }
        Dispatch::ToggleMute(what) => {
            (headset.sink)(MediaCommand::ToggleMute);
            headset.record(&format!("remote:{command}"), what);
        }
        Dispatch::Unchanged => headset.record(&format!("remote:{command}"), "ya estaba así"),
        Dispatch::NoCall => headset.record(&format!("remote:{command}"), "sin llamada: solo registrado"),
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
    let now_playing = active && N::NOW_PLAYING;
    let title = snapshot.title.trim().to_owned();
    let (mic, in_call) = (snapshot.mic_enabled, snapshot.ready && snapshot.joined);
    let next = N::NEXT_TRACK;
    let app2 = app.clone();
    let _ =
        app.run_on_main_thread(move || apply::<N>(&app2, Applied { active, now_playing, in_call, mic, next }, &title));
}

/// Whether the gesture is heard now (during a call; from launch where [`Os::MUTE_GESTURE_FOR_LIFE`]).
static GESTURE_ON: Mutex<bool> = Mutex::new(false);
/// Whether a call holds the system's input mute in step with its own (so its end gives it back unmuted).
static GESTURE_HELD: Mutex<bool> = Mutex::new(false);
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

/// The commands the app answers ([`registered_commands`]: the single press's, and the double press's `nextTrackCommand` where
/// it is registered, [`Os::NEXT_TRACK`]), each with its name.
fn commands(next: bool) -> Vec<(Retained<MPRemoteCommand>, &'static str)> {
    registered_commands(next).into_iter().map(|name| (command_named(name), name)).collect()
}

/// The command of the shared command center that `name` ([`ALL_COMMANDS`]) names.
fn command_named(name: &str) -> Retained<MPRemoteCommand> {
    // SAFETY: the shared command center exists for the process's lifetime; each subclass is a command.
    let center = unsafe { MPRemoteCommandCenter::sharedCommandCenter() };
    unsafe {
        match name {
            "play" => center.playCommand(),
            "pause" => center.pauseCommand(),
            "toggle" => center.togglePlayPauseCommand(),
            "stop" => center.stopCommand(),
            "next" => center.nextTrackCommand(),
            "previous" => center.previousTrackCommand(),
            "skipForward" => Retained::into_super(center.skipForwardCommand()),
            "skipBackward" => Retained::into_super(center.skipBackwardCommand()),
            "seekForward" => center.seekForwardCommand(),
            "seekBackward" => center.seekBackwardCommand(),
            "changePlaybackPosition" => Retained::into_super(center.changePlaybackPositionCommand()),
            "changePlaybackRate" => Retained::into_super(center.changePlaybackRateCommand()),
            "changeRepeatMode" => Retained::into_super(center.changeRepeatModeCommand()),
            "changeShuffleMode" => Retained::into_super(center.changeShuffleModeCommand()),
            "rating" => Retained::into_super(center.ratingCommand()),
            "like" => Retained::into_super(center.likeCommand()),
            "dislike" => Retained::into_super(center.dislikeCommand()),
            "bookmark" => Retained::into_super(center.bookmarkCommand()),
            "enableLanguageOption" => center.enableLanguageOptionCommand(),
            "disableLanguageOption" => center.disableLanguageOptionCommand(),
            other => unreachable!("no command is named {other}: see ALL_COMMANDS"),
        }
    }
}

fn gesture_api() -> bool {
    AnyClass::get(c"AVAudioApplication").is_some()
}

/// Every command of the shared command center, by name ([`ALL_COMMANDS`]): the app registers only [`commands`], but any of
/// them with a target, or enabled, keeps the app the Now Playing app as much (HANDOVER-live-activity R2-6), so all of them
/// are let go.
fn every_command() -> Vec<(Retained<MPRemoteCommand>, &'static str)> {
    ALL_COMMANDS.into_iter().map(|name| (command_named(name), name)).collect()
}

/// Whether the command handlers are registered now: only while Now Playing shows the call. A handler registered is what
/// makes the system take the app for the Now Playing app (iOS keeps an empty card on the Lock Screen for it, even with no
/// Now Playing info and every command disabled), so none is registered at launch, and they go when Now Playing does.
/// Only touched on the main thread.
static REGISTERED: Mutex<bool> = Mutex::new(false);

/// What the last [`apply`] had: whether a call was active, and whether Now Playing showed it. None before the first.
static LAST: Mutex<Option<(bool, bool)>> = Mutex::new(None);

/// At launch: no handlers yet (see [`REGISTERED`]); they are registered when a call is shown. Where the mute gesture is heard
/// for the process's life, it starts now; the app log says whether it did.
fn setup<N: Os>(app: &AppHandle) {
    release::<N>();
    if N::MUTE_GESTURE_FOR_LIFE {
        let on = gesture_api() && N::listen_for_mute_gesture(app);
        *GESTURE_ON.lock().unwrap() = on;
        log::info!("headset: press to mute {}", if on { "opted in" } else { "unavailable (needs iOS 17)" });
    }
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

/// Whether the double press's `nextTrackCommand` has its handler now. Only touched on the main thread.
static NEXT_REGISTERED: Mutex<bool> = Mutex::new(false);

/// Adds the handler of the command `name`.
fn add_handler(app: &AppHandle, command: &MPRemoteCommand, name: &'static str) {
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

/// Registers the handlers of the commands, once; the double press's only where `next` says ([`Applied::next`]).
fn register<N: Os>(app: &AppHandle, next: bool) {
    for (command, name) in commands(next) {
        add_handler(app, &command, name);
    }
    *NEXT_REGISTERED.lock().unwrap() = next;
}

/// The double press's command follows whether the person wants it, while the others stay registered: its handler goes or
/// comes without the call's card being let go of.
fn follow_next(app: &AppHandle, next: bool) {
    let mut registered = NEXT_REGISTERED.lock().unwrap();
    if *registered == next {
        return;
    }
    let command = command_named("next");
    // SAFETY: main thread; `nil` removes every target of the command.
    unsafe {
        if next {
            add_handler(app, &command, "next");
            command.setEnabled(true);
        } else {
            command.removeTarget(None);
            command.setEnabled(false);
        }
    }
    *registered = next;
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
    *NEXT_REGISTERED.lock().unwrap() = false;
    N::end_remote_control_events();
}

/// What [`apply`] shows: whether a call is active, whether Now Playing shows it, whether it is joined, whether the microphone
/// is open, and whether the double press's command is registered ([`Os::NEXT_TRACK`]).
struct Applied {
    active: bool,
    now_playing: bool,
    in_call: bool,
    mic: bool,
    next: bool,
}

fn apply<N: Os>(app: &AppHandle, shown: Applied, title: &str) {
    let Applied { active, now_playing, in_call, mic: mic_enabled, next } = shown;
    // Let go of Now Playing whenever it is not to show the call and something changed: a call starting (the report that
    // starts the call's service; on iOS, every call, which shows no Now Playing) and a call ending. What was there at a
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
        register::<N>(app, next);
        *REGISTERED.lock().unwrap() = true;
        for (command, _) in commands(next) {
            // SAFETY: main thread; plain property set.
            unsafe { command.setEnabled(true) };
        }
    }
    if now_playing {
        follow_next(app, next);
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

/// AirPods / Beats mute gesture (macOS 14+, iOS 17+, where `AVAudioApplication` exists): heard while active (or for the process's
/// life, [`Os::MUTE_GESTURE_FOR_LIFE`]), the system's input mute kept in step with the microphone during a call and given back
/// unmuted at its end ([`gesture_step`]).
fn gesture<N: Os>(app: &AppHandle, active: bool, in_call: bool, mic_enabled: bool) {
    if !gesture_api() {
        return;
    }
    // SAFETY: the class exists (checked above); main thread.
    unsafe {
        let shared = AVAudioApplication::sharedInstance();
        let mut on = GESTURE_ON.lock().unwrap();
        let mut held = GESTURE_HELD.lock().unwrap();
        let step = gesture_step(N::MUTE_GESTURE_FOR_LIFE, *on, active, *held);
        if step.listen {
            *on = N::listen_for_mute_gesture(app);
        }
        if step.stop {
            N::stop_listening_for_mute_gesture();
            *on = false;
        }
        if step.give_back {
            set_muted(&shared, false);
        }
        *held = active && *on;
        if *on && in_call && shared.isInputMuted() == mic_enabled {
            // Keep the system's (and the AirPods') idea of "muted" in step with the call's.
            set_muted(&shared, !mic_enabled);
        }
    }
}
