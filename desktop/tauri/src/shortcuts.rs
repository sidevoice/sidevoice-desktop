//! The global shortcuts, in the running app (docs/SHORTCUTS.md, docs/BRIDGE.md → "The shortcuts"): `host.shortcuts` onto
//! the settings, the system's hot keys and a listener of single keys, the gestures made of their events, and the safety net
//! for a hold that never ends. The rules are core `shortcuts`'; the operating system is `platform::shortcut_access`,
//! `platform::key_capture` and `platform::session`.
//!
//! Locks: `change` serialises changes and is only ever taken off the main thread (registering waits for it); `current` is
//! what the hot key handler and the listener read, held for a moment and never across a call into the system.

use crate::platform::key_capture::{self, KeyEvent};
use crate::platform::{session, shortcut_access};
use crate::{refusal, AppState};
use serde_json::{json, Value};
use sidevoice_desktop_core::bridge::Command;
use sidevoice_desktop_core::settings::{GlobalShortcut, Registration, Settings};
use sidevoice_desktop_core::shortcuts::{
    self, Action, Edge, Effect, Headset, HeadsetPress, Heard, Held, Hold, Machine, Parsed, Permission, PermissionKind,
    PermissionState, Recorded, Recorder, Release, Status, Watch, WATCH_TICK,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

/// How long a recording waits for a key before it gives up (the person walked away).
const RECORD_WAIT: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(crate) struct Shortcuts {
    /// Held across a change of any shortcut, from keeping it to registering it, so what is kept and what is registered never
    /// differ. Only off the main thread: registering waits for it.
    change: Mutex<()>,
    /// The hot keys the app holds. Only under `change`.
    held: Mutex<Held>,
    /// The listener of single keys, while one is set and the system allows it. Only under `change`.
    listener: Mutex<Option<key_capture::Listener>>,
    /// How many recordings are on: while one is, the hot keys and the listener are let go of, or the key being recorded
    /// would fire what it is bound to. Only under `change`.
    recordings: Mutex<usize>,
    current: Mutex<Current>,
}

#[derive(Default)]
struct Current {
    /// What the system took of each, in [`Action::ALL`] order, as `get` tells it.
    shortcuts: [GlobalShortcut; 3],
    permission: Option<Permission>,
    headset: Option<Headset>,
    /// What each is bound to, and the gesture it makes.
    parsed: [Option<Parsed>; 3],
    machines: [Option<Machine>; 3],
    hold: Hold,
}

/// Starts the shortcuts: the plugin's hot keys come to [`hot_key`].
pub(crate) fn setup(app: &AppHandle) {
    app.manage(Shortcuts::default());
    shortcut_access::configure(app, Arc::new(hot_key));
}

fn setting(settings: &Settings, action: Action) -> &str {
    match action {
        Action::Mute => &settings.mute_shortcut,
        Action::HoldToTalk => &settings.hold_to_talk_shortcut,
        Action::OneTurn => &settings.one_turn_shortcut,
    }
}

fn setting_mut(settings: &mut Settings, action: Action) -> &mut String {
    match action {
        Action::Mute => &mut settings.mute_shortcut,
        Action::HoldToTalk => &mut settings.hold_to_talk_shortcut,
        Action::OneTurn => &mut settings.one_turn_shortcut,
    }
}

/// What is kept, or a first run's defaults.
fn stored(app: &AppHandle) -> Settings {
    app.state::<AppState>()
        .settings
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| Settings::first_run(shortcut_access::default_mute()))
}

/// What a kept shortcut asks for: what cannot be read (a file from another build, edited) is no shortcut.
fn parse_kept(action: Action, text: &str) -> Option<Parsed> {
    match shortcuts::parse(text) {
        Some(Some(parsed)) if shortcuts::suits(action, &parsed) && shortcut_access::accepts(&parsed) => Some(parsed),
        Some(None) => None,
        _ => {
            log::warn!("the {} shortcut {text:?} is not one the app can register: none", action.name());
            None
        }
    }
}

/// Brings the system to what the settings say, then tells the page and the card. Startup, and every change.
pub(crate) fn apply(app: &AppHandle) {
    let runtime = app.state::<Shortcuts>();
    let _one_at_a_time = runtime.change.lock().unwrap();
    if *runtime.recordings.lock().unwrap() == 0 {
        apply_locked(app, &runtime);
    }
}

fn apply_locked(app: &AppHandle, runtime: &Shortcuts) {
    let settings = stored(app);
    let language = app.state::<AppState>().language;
    let parsed: [Option<Parsed>; 3] = Action::ALL.map(|action| parse_kept(action, setting(&settings, action)));
    // The same single key and way of pressing it, set for two actions: the first holds it. (Hot keys are told apart by the
    // diff itself, which sees the same keys however they are written.)
    let duplicate: [bool; 3] = std::array::from_fn(|i| {
        parsed[i]
            .as_ref()
            .is_some_and(|p| p.listened().is_some() && parsed[..i].iter().flatten().any(|earlier| earlier == p))
    });
    let wanted: [Option<String>; 3] =
        std::array::from_fn(|i| if duplicate[i] { None } else { parsed[i].as_ref().and_then(Parsed::hot_key) });
    let taken = shortcut_access::replace(app, &mut runtime.held.lock().unwrap(), &wanted);

    // The listener: only while a single key that needs it is set and the system allows it (macOS: Input Monitoring, which is
    // asked for only on the person's Allow). Nothing is created, and nothing asked, while only combinations are set.
    let access = key_capture::access();
    let needed = (0..3).any(|i| !duplicate[i] && parsed[i].as_ref().is_some_and(|p| p.listened().is_some()));
    let listening = {
        let mut listener = runtime.listener.lock().unwrap();
        if needed && key_capture::listens() && access != Some(false) {
            if listener.is_none() {
                let target = app.clone();
                *listener = key_capture::listen(Arc::new(move |event| key_event(&target, event)));
            }
        } else {
            *listener = None;
        }
        listener.is_some()
    };

    let symbols = shortcut_access::symbol_labels();
    let mut all: [GlobalShortcut; 3] = Default::default();
    for action in Action::ALL {
        let i = action.index();
        let Some(binding) = &parsed[i] else { continue };
        let (state, alternatives) = if duplicate[i] {
            (Registration::Refused, vec![])
        } else if binding.listened().is_some() {
            let state = if !key_capture::listens() {
                Registration::Unavailable
            } else if access == Some(false) {
                Registration::NeedsPermission
            } else if listening {
                Registration::On
            } else {
                Registration::Unavailable
            };
            (state, vec![])
        } else {
            taken[i].clone().map_or((Registration::Off, vec![]), |t| (t.state, t.alternatives))
        };
        all[i] = GlobalShortcut::new(&binding.accelerator(), state, alternatives, symbols, language);
    }
    let permission = (key_capture::listens() && parsed.iter().flatten().any(Parsed::is_single)).then(|| Permission {
        kind: PermissionKind::InputMonitoring,
        state: if !needed || access == Some(true) {
            PermissionState::Granted
        } else if settings.input_monitoring_asked {
            PermissionState::Denied
        } else {
            PermissionState::NotAsked
        },
    });
    let headset =
        sidevoice_shared_tauri::platform::media_controls::available().then(|| Headset::new(settings.headset_press));

    // What each does: a binding that did not change keeps its machine (a hold in progress goes on), one that did starts over.
    let mut let_go = false;
    {
        let mut current = runtime.current.lock().unwrap();
        for action in Action::ALL {
            let i = action.index();
            let active = parsed[i].is_some() && all[i].state == Registration::On;
            let same = current.parsed[i] == parsed[i] && current.machines[i].is_some() == active;
            if !same {
                if action == Action::HoldToTalk && current.hold.is_on() {
                    let_go = true;
                }
                current.machines[i] =
                    parsed[i].as_ref().filter(|_| active).map(|p| Machine::new(shortcuts::gesture(action, p)));
            }
        }
        current.parsed = parsed;
        current.shortcuts = all;
        current.permission = permission;
        current.headset = headset;
    }
    if let_go {
        release(app, Release::Rebound);
    }
    notify(app);
}

/// The status as `host.shortcuts.get` tells it. A permission the person has just turned on in System Settings is found here.
pub(crate) fn status(app: &AppHandle) -> Status {
    let waiting = app
        .state::<Shortcuts>()
        .current
        .lock()
        .unwrap()
        .shortcuts
        .iter()
        .any(|s| s.state == Registration::NeedsPermission);
    if waiting && key_capture::access() == Some(true) {
        apply(app);
    }
    snapshot(app)
}

/// The status now. VoiceOver can be turned on or off at any time.
fn snapshot(app: &AppHandle) -> Status {
    let runtime = app.state::<Shortcuts>();
    let current = runtime.current.lock().unwrap();
    let mut all = current.shortcuts.clone();
    for shortcut in &mut all {
        let accelerator = shortcut.accelerator.as_deref().unwrap_or_default();
        shortcut.voice_over = shortcut.state == Registration::On && shortcut_access::reserved_chord_active(accelerator);
    }
    Status {
        shortcuts: shortcuts::Shortcuts::from_array(all),
        permission: current.permission,
        headset: current.headset,
    }
}

/// One shortcut as the system took it (the card's tooltip and `host.muteShortcut` read the mute one).
pub(crate) fn shortcut_of(app: &AppHandle, action: Action) -> GlobalShortcut {
    snapshot(app).shortcuts.get(action).clone()
}

/// Tells the room page and the call controls card that the status changed.
fn notify(app: &AppHandle) {
    if let Some(window) = crate::room_window(app) {
        let status = json!(snapshot(app));
        let _ = window.eval(format!("window.__sidevoiceHost && window.__sidevoiceHost.shortcutsEvent({status})"));
    }
    crate::call_controls::update(app);
}

// ---------------------------------------------------------------------------------------------------------------------
// The page's commands
// ---------------------------------------------------------------------------------------------------------------------

/// `host.shortcuts.set`: keeps `text` (empty: none) for `action` and registers it. Refuses `shortcut_invalid` for what is not
/// a shortcut the system can read, or not one `action` can be done with. What the system then says of it (taken by another
/// app, set for two actions) is the answer's `state`, with the keys it offers instead.
pub(crate) fn set(app: &AppHandle, action: Action, text: &str) -> Result<GlobalShortcut, Value> {
    let invalid = || refusal("shortcut_invalid", format!("Not a shortcut: {text}"));
    let parsed = shortcuts::parse(text).ok_or_else(invalid)?;
    if let Some(parsed) = &parsed {
        if !shortcuts::suits(action, parsed) || !shortcut_access::accepts(parsed) {
            return Err(invalid());
        }
    }
    let kept = parsed.map(|p| p.accelerator()).unwrap_or_default();
    {
        let runtime = app.state::<Shortcuts>();
        let _one_at_a_time = runtime.change.lock().unwrap();
        crate::change_settings(app, |s| *setting_mut(s, action) = kept).map_err(|e| refusal("internal", e))?;
        if *runtime.recordings.lock().unwrap() == 0 {
            apply_locked(app, &runtime);
        }
    }
    Ok(shortcut_of(app, action))
}

/// `host.shortcuts.request`: the person's Allow for single keys (macOS: Input Monitoring). The system's prompt shows the
/// first time; after a refusal the person has to turn it on in System Settings, which this opens. Never called by the app.
pub(crate) fn request(app: &AppHandle, permission: Option<&str>) -> Result<Status, Value> {
    match permission {
        None | Some("inputMonitoring") if key_capture::listens() => {}
        None | Some("inputMonitoring") => {
            return Err(refusal("unsupported", "single keys need no permission on this system"))
        }
        Some(other) => return Err(refusal("bad_request", format!("no such permission: {other}"))),
    }
    if key_capture::access() != Some(true) {
        let asked = stored(app).input_monitoring_asked;
        crate::change_settings(app, |s| s.input_monitoring_asked = true).map_err(|e| refusal("internal", e))?;
        if !key_capture::request_access() && asked {
            if let Some(url) = key_capture::settings_url() {
                app.opener().open_url(url, None::<&str>).map_err(|e| refusal("internal", e.to_string()))?;
            }
        }
        watch_for_permission(app);
    }
    apply(app);
    Ok(status(app))
}

/// The system tells nothing when the person turns the permission on: the app looks, for a minute, and starts listening.
fn watch_for_permission(app: &AppHandle) {
    let app = app.clone();
    let _ = std::thread::Builder::new().name("input-monitoring-watch".into()).spawn(move || {
        for _ in 0..60 {
            std::thread::sleep(Duration::from_secs(1));
            if key_capture::access() == Some(true) {
                apply(&app);
                return;
            }
        }
    });
}

/// `host.shortcuts.setHeadset`: what a headset's single press does. The path the app has (the system's play/pause) cannot
/// send a double press, and the app leaves a headset's buttons to other apps outside a call, so those are refused.
pub(crate) fn set_headset(
    app: &AppHandle,
    press: Option<&str>,
    double: Option<&Value>,
    outside: Option<&str>,
) -> Result<Status, Value> {
    if !sidevoice_shared_tauri::platform::media_controls::available() {
        return Err(refusal("unsupported", "this system has no headset path the app owns"));
    }
    if double.is_some_and(|value| !value.is_null()) {
        return Err(refusal("bad_request", "this headset path cannot send a double press"));
    }
    if outside.is_some_and(|value| value != "leave") {
        return Err(refusal("bad_request", "the app leaves a headset's buttons to other apps outside a call"));
    }
    if let Some(press) = press {
        let press =
            HeadsetPress::parse(press).ok_or_else(|| refusal("bad_request", format!("no such press: {press}")))?;
        let runtime = app.state::<Shortcuts>();
        let _one_at_a_time = runtime.change.lock().unwrap();
        crate::change_settings(app, |s| s.headset_press = press).map_err(|e| refusal("internal", e))?;
        apply_locked(app, &runtime);
    }
    Ok(status(app))
}

/// The headset's single press (the system's play/pause, or the AirPods gesture) does what the person chose.
pub(crate) fn headset_press(app: &AppHandle) {
    match stored(app).headset_press {
        HeadsetPress::Mute => crate::send(app, Command::ToggleMute),
        HeadsetPress::OneTurn => crate::send(app, Command::OneTurn),
        HeadsetPress::Skip => crate::send(app, Command::SkipReply),
        HeadsetPress::None => {}
    }
}

/// `host.shortcuts.record`: the app hears the next key itself, Fn included, and answers with the accelerator, or `None` for
/// Escape (or no key for a minute). Meanwhile the hot keys and the listener are let go of, so the keys recorded do not fire
/// what they are bound to.
pub(crate) async fn record(app: AppHandle, action: Action) -> Result<Option<String>, Value> {
    if !key_capture::captures() {
        return Err(refusal("unsupported", "this system cannot capture a key for the page"));
    }
    let (done, mut result) = tokio::sync::oneshot::channel::<Option<String>>();
    let recorder = Arc::new(Mutex::new(Recorder::new(action)));
    let finish = Arc::new(Mutex::new(Some(done)));
    pause(&app);
    let heard = {
        let (recorder, finish) = (recorder.clone(), finish.clone());
        Arc::new(move |heard: Heard| {
            if let Recorded::Done(answer) = recorder.lock().unwrap().hear(heard, Instant::now()) {
                if let Some(done) = finish.lock().unwrap().take() {
                    let _ = done.send(answer);
                }
            }
        })
    };
    let Some(capture) = key_capture::capture(&app, heard) else {
        resume(&app);
        return Err(refusal("unsupported", "this system cannot capture a key for the page"));
    };
    let deadline = tokio::time::Instant::now() + RECORD_WAIT;
    let answer = loop {
        tokio::select! {
            got = &mut result => break got.unwrap_or(None),
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                // A lone tap waits to see whether a second one follows.
                if let Recorded::Done(answer) = recorder.lock().unwrap().settle(Instant::now()) {
                    break answer;
                }
                if tokio::time::Instant::now() >= deadline {
                    break None;
                }
            }
        }
    };
    drop(capture);
    resume(&app);
    Ok(answer)
}

/// Lets go of the hot keys and the listener while a key is being recorded.
fn pause(app: &AppHandle) {
    let runtime = app.state::<Shortcuts>();
    let _one_at_a_time = runtime.change.lock().unwrap();
    let mut recordings = runtime.recordings.lock().unwrap();
    *recordings += 1;
    if *recordings == 1 {
        shortcut_access::replace(app, &mut runtime.held.lock().unwrap(), &[None, None, None]);
        *runtime.listener.lock().unwrap() = None;
    }
}

/// Takes them back, as the settings say.
fn resume(app: &AppHandle) {
    let runtime = app.state::<Shortcuts>();
    let _one_at_a_time = runtime.change.lock().unwrap();
    let mut recordings = runtime.recordings.lock().unwrap();
    *recordings = recordings.saturating_sub(1);
    if *recordings == 0 {
        drop(recordings);
        apply_locked(app, &runtime);
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// What the keys do
// ---------------------------------------------------------------------------------------------------------------------

/// A hot key went down or up.
fn hot_key(app: &AppHandle, action: Action, edge: Edge) {
    feed(app, action, edge);
}

/// What a listener heard: the single key's own edges to the action bound to it, and another key to every pending tap.
fn key_event(app: &AppHandle, event: KeyEvent) {
    let runtime = app.state::<Shortcuts>();
    let targets: Vec<(Action, Edge)> = {
        let current = runtime.current.lock().unwrap();
        Action::ALL
            .into_iter()
            .filter_map(|action| match (&current.parsed[action.index()], event) {
                (Some(Parsed::Single { key, .. }), KeyEvent::Single { key: heard, down }) if *key == heard => {
                    Some((action, if down { Edge::Down } else { Edge::Up }))
                }
                (Some(binding), KeyEvent::Other) if binding.listened().is_some() => Some((action, Edge::Other)),
                _ => None,
            })
            .collect()
    };
    for (action, edge) in targets {
        feed(app, action, edge);
    }
}

fn feed(app: &AppHandle, action: Action, edge: Edge) {
    let effect = {
        let runtime = app.state::<Shortcuts>();
        let mut current = runtime.current.lock().unwrap();
        current.machines[action.index()].as_mut().and_then(|machine| machine.on(edge, Instant::now()))
    };
    if let Some(effect) = effect {
        act(app, action, effect);
    }
}

fn act(app: &AppHandle, action: Action, effect: Effect) {
    match (action, effect) {
        (Action::Mute, Effect::Fire) => crate::send(app, Command::ToggleMute),
        (Action::OneTurn, Effect::Fire) => crate::send(app, Command::OneTurn),
        (Action::HoldToTalk, Effect::HoldStart) => begin_hold(app),
        (Action::HoldToTalk, Effect::HoldEnd) => end_hold(app),
        _ => {}
    }
}

fn begin_hold(app: &AppHandle) {
    let generation = app.state::<Shortcuts>().current.lock().unwrap().hold.begin();
    let Some(generation) = generation else { return };
    crate::send(app, Command::HoldStart);
    watch(app, generation);
}

fn end_hold(app: &AppHandle) {
    let was_on = app.state::<Shortcuts>().current.lock().unwrap().hold.end();
    if was_on {
        crate::send(app, Command::HoldEnd);
    }
}

/// The app lets go of a hold the person's hand did not: the page is told as if the key had come up, and a release that comes
/// late is nothing.
fn release(app: &AppHandle, why: Release) {
    let was_on = {
        let runtime = app.state::<Shortcuts>();
        let mut current = runtime.current.lock().unwrap();
        if let Some(machine) = current.machines[Action::HoldToTalk.index()].as_mut() {
            machine.cancel();
        }
        current.hold.end()
    };
    if was_on {
        log::info!("hold to talk let go: {why:?}");
        crate::send(app, Command::HoldEnd);
    }
}

/// The call ended (or the room went away): a hold has nothing left to talk to.
pub(crate) fn call_changed(app: &AppHandle, joined: bool) {
    if !joined {
        release(app, Release::CallEnded);
    }
}

/// Looks at a hold every second while it lasts: past a minute, after a sleep, or behind a locked screen, it is let go.
fn watch(app: &AppHandle, generation: u64) {
    let app = app.clone();
    let _ = std::thread::Builder::new().name("hold-watch".into()).spawn(move || {
        let mut watch = Watch::new(Instant::now(), SystemTime::now());
        loop {
            std::thread::sleep(WATCH_TICK);
            if !app.state::<Shortcuts>().current.lock().unwrap().hold.is_current(generation) {
                return;
            }
            if let Some(why) = watch.look(Instant::now(), SystemTime::now(), session::screen_locked()) {
                // Only if it is still this hold: the person may have let go between the look and now.
                if app.state::<Shortcuts>().current.lock().unwrap().hold.is_current(generation) {
                    release(&app, why);
                }
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidevoice_desktop_core::shortcuts::{Gesture, STUCK_HOLD};

    #[test]
    fn a_kept_shortcut_that_cannot_be_registered_is_none_not_an_error() {
        assert_eq!(parse_kept(Action::Mute, ""), None);
        assert_eq!(parse_kept(Action::Mute, "Ctrl+UnknownKey"), None);
        assert_eq!(parse_kept(Action::HoldToTalk, "Fn:tap"), None, "hold to talk is held");
        assert_eq!(parse_kept(Action::Mute, "RightControl:hold"), None);
        assert!(parse_kept(Action::HoldToTalk, "Fn:hold").is_some());
        assert!(parse_kept(Action::OneTurn, "RightControl:double").is_some());
        assert!(parse_kept(Action::Mute, "Ctrl+Alt+Shift+M").is_some());
    }

    #[test]
    fn the_gesture_of_each_action_is_what_the_person_chose() {
        let of = |action, text: &str| shortcuts::gesture(action, &shortcuts::parse(text).unwrap().unwrap());
        assert_eq!(of(Action::HoldToTalk, "Ctrl+Alt+H"), Gesture::Hold);
        assert_eq!(of(Action::Mute, "Ctrl+Alt+M"), Gesture::Press);
        assert_eq!(of(Action::OneTurn, "RightControl:double"), Gesture::Double);
        assert_eq!(STUCK_HOLD, Duration::from_secs(60));
    }
}
