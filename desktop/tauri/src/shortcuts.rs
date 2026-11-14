//! The global shortcuts, in the running app (docs/SHORTCUTS.md, docs/BRIDGE.md → "The shortcuts"): `host.shortcuts` onto
//! the settings, the system's hot keys and a listener of single keys, the gestures made of their events, and the safety net
//! for a hold that never ends. The rules are core `shortcuts`'; the operating system is `platform::shortcut_access`,
//! `platform::key_capture` and `platform::session`.
//!
//! Locks: `change` serialises changes and is only ever taken off the main thread (registering waits for it); `current` is
//! what the hot key handler and the listener read, held for a moment and never across a call into the system.

use crate::platform::key_capture;
use crate::platform::{session, shortcut_access};
use crate::{refusal, AppState};
use serde_json::{json, Value};
use sidevoice_desktop_core::bridge::Command;
use sidevoice_desktop_core::settings::{GlobalShortcut, Refusal, Registration, Settings};
use sidevoice_desktop_core::shortcuts::{
    self, Action, Carried, Edge, Effect, Headset, HeadsetChange, HeadsetPress, Heard, Held, Hint, Hold, KeyMachine,
    KeyReach, Listened, Machine, Parsed, Permission, PermissionKind, PermissionState, Pulse, Recorded, Recorder,
    Release, SingleKey, Status, Watch, WATCH_TICK,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

/// How long a recording waits for a key before it gives up (the person walked away): the page is told, as an error.
const RECORD_WAIT: Duration = Duration::from_secs(15);

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
    /// What each is bound to.
    parsed: [Option<Parsed>; 3],
    /// The gesture of each combination (a single key is in `keys`).
    machines: [Option<Machine>; 3],
    /// The single keys in use, each with what it carries and its machine: one key, however many ways it is pressed.
    keys: Vec<KeySlot>,
    hold: Hold,
}

struct KeySlot {
    key: SingleKey,
    carried: Carried,
    machine: KeyMachine,
}

/// Starts the shortcuts: the plugin's hot keys come to [`hot_key`].
pub(crate) fn setup(app: &AppHandle) {
    app.manage(Shortcuts::default());
    shortcut_access::configure(app, Arc::new(hot_key));
}

/// What is kept, or a first run's defaults.
fn stored(app: &AppHandle) -> Settings {
    app.state::<AppState>().settings.lock().unwrap().clone().unwrap_or_else(Settings::first_run)
}

/// What a kept shortcut asks for: what cannot be read (a file from another build, edited) is no shortcut.
fn parse_kept(action: Action, text: &str) -> Option<Parsed> {
    match shortcuts::parse(text) {
        Some(Some(parsed)) if shortcuts::suits(action, &parsed) && shortcut_access::accepts(&parsed) => Some(parsed),
        Some(None) => None,
        _ => {
            tracing::warn!(action = action.name(), "shortcut not one the app can register");
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
    let default_mute = shortcut_access::default_mute();
    let parsed: [Option<Parsed>; 3] =
        Action::ALL.map(|action| parse_kept(action, &settings.shortcut_or_default(action, default_mute)));
    // The same physical single key set for two actions in the same way: the first holds it. The same key in different ways
    // coexist (one machine tells them apart by time), and an F-key has one hot key between them, the first action's.
    let duplicate = shortcuts::duplicates(&parsed);
    let rides = shortcuts::rides(&parsed, &duplicate);
    let wanted: [Option<String>; 3] = std::array::from_fn(|i| {
        if duplicate[i].is_some() || rides[i].is_some() {
            None
        } else {
            parsed[i].as_ref().and_then(Parsed::hot_key)
        }
    });
    let taken = shortcut_access::replace(app, &mut runtime.held.lock().unwrap(), &wanted);

    // The listener: only while a single key that needs it is set and the system allows it (macOS: Input Monitoring, which is
    // asked for only on the person's Allow). Nothing is created, and nothing asked, while only combinations are set.
    let access = key_capture::access();
    let needed = (0..3).any(|i| duplicate[i].is_none() && parsed[i].as_ref().is_some_and(|p| p.listened().is_some()));
    let listening = {
        let mut listener = runtime.listener.lock().unwrap();
        if needed && key_capture::listens() && access != Some(false) {
            if listener.is_none() {
                let target = app.clone();
                *listener = key_capture::listen(Arc::new(move |heard| key_event(&target, heard)));
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
        let (state, alternatives, refusal) = if let Some(holder) = duplicate[i] {
            let offered = shortcut_access::alternatives(app, &wanted, action);
            (Registration::Refused, offered, Some(Refusal::Duplicate { action: holder }))
        } else if let Some(owner) = rides[i] {
            // The same hot key as the earlier action's, pressed in another way: it is taken, or not, with it.
            let of = &all[owner.index()];
            (of.state, of.alternatives.clone(), of.refusal)
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
            (state, vec![], None)
        } else {
            match taken[i].clone() {
                None => (Registration::Off, vec![], None),
                Some(t) => {
                    let refusal = (t.state == Registration::Refused)
                        .then(|| t.holder.map_or(Refusal::Taken, |action| Refusal::Duplicate { action }));
                    (t.state, t.alternatives, refusal)
                }
            }
        };
        all[i] = GlobalShortcut {
            refusal,
            ..GlobalShortcut::new(&binding.accelerator(), state, alternatives, symbols, language)
        };
    }
    // What the system's default is for each, and whether the person never chose: the page keeps neither.
    for action in Action::ALL {
        let default = shortcuts::default_of(action, default_mute);
        let shortcut = &mut all[action.index()];
        shortcut.is_default = settings.shortcut(action).is_none();
        shortcut.default = (!default.is_empty()).then(|| default.to_owned());
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
    let headset = sidevoice_shared_tauri::platform::media_controls::available()
        .then(|| Headset::new(settings.headset_press, settings.headset_double_press));

    // What each does: a binding that did not change keeps its machine (a hold in progress goes on), one that did starts over.
    let active: [bool; 3] = std::array::from_fn(|i| parsed[i].is_some() && all[i].state == Registration::On);
    let keys = shortcuts::single_keys(&parsed, &active.map(|on| !on));
    let mut let_go = false;
    {
        let mut current = runtime.current.lock().unwrap();
        for action in Action::ALL {
            let i = action.index();
            let combo = active[i] && matches!(parsed[i], Some(Parsed::Combo(_)));
            let same = current.parsed[i] == parsed[i] && current.machines[i].is_some() == combo;
            if !same {
                if action == Action::HoldToTalk && current.hold.is_on() && current.machines[i].is_some() {
                    let_go = true;
                }
                current.machines[i] =
                    parsed[i].as_ref().filter(|_| combo).and_then(|p| shortcuts::gesture(action, p)).map(Machine::new);
            }
        }
        // A key whose ways did not change keeps its machine, so a press in progress goes on.
        let before = std::mem::take(&mut current.keys);
        let held_before = before.iter().find(|slot| slot.carried.hold.is_some()).map(|slot| slot.key);
        if current.hold.is_on()
            && held_before.is_some_and(|key| !keys.iter().any(|(k, c)| *k == key && c.hold.is_some()))
        {
            let_go = true;
        }
        current.keys = keys
            .into_iter()
            .map(|(key, carried)| {
                let ways = carried.ways();
                let machine = before
                    .iter()
                    .find(|slot| slot.key == key && slot.machine.ways() == ways)
                    .map_or_else(|| KeyMachine::new(ways), |slot| slot.machine.clone());
                KeySlot { key, carried, machine }
            })
            .collect();
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
        shortcut.layout_bound = shortcut.state == Registration::On
            && shortcuts::hot_key_reach(shortcut_access::platform(), accelerator) == KeyReach::ByLayout;
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
        if !shortcuts::suits(action, parsed) {
            return Err(invalid());
        }
        // A key this system cannot register as itself (the ISO `<>` key, a keypad key Windows maps to another): refused,
        // and never bound as another key. The page says why.
        if parsed
            .hot_key()
            .is_some_and(|keys| shortcuts::hot_key_reach(shortcut_access::platform(), &keys) == KeyReach::Never)
        {
            return Err(refusal("shortcut_unsupported", format!("This system cannot register that key: {text}")));
        }
        if !shortcut_access::accepts(parsed) {
            return Err(invalid());
        }
    }
    let kept = parsed.map(|p| p.accelerator()).unwrap_or_default();
    choose(app, action, Some(kept))
}

/// `host.shortcuts.restore`: puts `action` back on this system's default, as if the person never chose.
pub(crate) fn restore(app: &AppHandle, action: Action) -> Result<GlobalShortcut, Value> {
    choose(app, action, None)
}

/// Keeps what the person chose for `action` (empty: none; `None`: the default) and registers it.
fn choose(app: &AppHandle, action: Action, kept: Option<String>) -> Result<GlobalShortcut, Value> {
    {
        let runtime = app.state::<Shortcuts>();
        let _one_at_a_time = runtime.change.lock().unwrap();
        crate::change_settings(app, |s| s.choose_shortcut(action, kept, shortcut_access::default_mute()))
            .map_err(|e| refusal("internal", e))?;
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

/// `host.shortcuts.setHeadset`: what a headset's single press and double press do. The app leaves a headset's buttons to
/// other apps outside a call, so `outsideCall: "join"` is refused.
pub(crate) fn set_headset(
    app: &AppHandle,
    press: Option<&str>,
    double: Option<&Value>,
    outside: Option<&str>,
) -> Result<Status, Value> {
    if !sidevoice_shared_tauri::platform::media_controls::available() {
        return Err(refusal("unsupported", "this system has no headset path the app owns"));
    }
    let change = HeadsetChange::parse(press, double, outside).map_err(|why| refusal("bad_request", why))?;
    if !change.is_empty() {
        let runtime = app.state::<Shortcuts>();
        let _one_at_a_time = runtime.change.lock().unwrap();
        crate::change_settings(app, |s| {
            s.headset_press = change.press.unwrap_or(s.headset_press);
            s.headset_double_press = change.double_press.unwrap_or(s.headset_double_press);
        })
        .map_err(|e| refusal("internal", e))?;
        apply_locked(app, &runtime);
        // The AirPods gesture follows the new choice during a call (whether the system is told it muted).
        crate::refresh_media_controls(app);
    }
    Ok(status(app))
}

/// Whether the headset's single press the person chose leaves the microphone alone ([`HeadsetPress::keeps_mic`]).
pub(crate) fn headset_press_keeps_mic(app: &AppHandle) -> bool {
    stored(app).headset_press.keeps_mic()
}

/// The headset's single press (the system's play/pause, or the AirPods gesture) does what the person chose.
pub(crate) fn headset_press(app: &AppHandle) {
    headset_does(app, stored(app).headset_press);
}

/// The headset's double press (the system's next track) does what the person chose.
pub(crate) fn headset_double_press(app: &AppHandle) {
    headset_does(app, stored(app).headset_double_press);
}

fn headset_does(app: &AppHandle, press: HeadsetPress) {
    match press {
        HeadsetPress::Mute => crate::send(app, Command::ToggleMute),
        HeadsetPress::OneTurn => crate::send(app, Command::OneTurn),
        HeadsetPress::Skip => crate::send(app, Command::SkipReply),
        HeadsetPress::None => {}
    }
}

/// `host.shortcuts.record`: the app hears the next key itself, Fn included, and answers with the accelerator, or `None` for
/// Escape. No key for [`RECORD_WAIT`] is the error `record_timeout`, not a cancel. A key it cannot take (a modifier on its own
/// where only the right-hand ones can be a key) is ignored, the page told why, and the recording goes on. Meanwhile the hot
/// keys and the listener are let go of, so the keys recorded do not fire what they are bound to.
///
/// Nothing here blocks a worker of the async runtime: setting the capture up and giving the keys back wait on the main thread
/// and on the shortcut locks, so both run on the blocking pool, the second also when the page goes away (the future is dropped).
pub(crate) async fn record(app: AppHandle, action: Action) -> Result<Option<String>, Value> {
    if !key_capture::captures() {
        return Err(refusal("unsupported", "this system cannot capture a key for the page"));
    }
    let (done, mut result) = tokio::sync::oneshot::channel::<Option<String>>();
    let recorder = Arc::new(Mutex::new(Recorder::on(action, key_capture::listens())));
    let finish = Arc::new(Mutex::new(Some(done)));
    let heard = {
        let (recorder, finish, app) = (recorder.clone(), finish.clone(), app.clone());
        Arc::new(move |heard: Heard| match recorder.lock().unwrap().hear(heard, Instant::now()) {
            Recorded::Done(answer) => {
                if let Some(done) = finish.lock().unwrap().take() {
                    let _ = done.send(answer);
                }
            }
            Recorded::Hint(hint) => tell_hint(&app, hint),
            Recorded::Pending => {}
        })
    };
    // One deadline for the whole recording, startup included: a stalled main thread must not hang Settings.
    let deadline = tokio::time::Instant::now() + RECORD_WAIT;
    let starting = app.clone();
    let session = match within(deadline, move || Recording::start(&starting, heard)).await {
        Ok(session) => session,
        Err(Late::Timeout) => return Err(refusal("record_timeout", "the recording did not start in time")),
        Err(Late::Failed(why)) => return Err(refusal("internal", why)),
    };
    let Some(_session) = session else {
        return Err(refusal("unsupported", "this system cannot capture a key for the page"));
    };
    loop {
        tokio::select! {
            got = &mut result => return Ok(got.unwrap_or(None)),
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                // A lone tap waits to see whether a second one follows.
                if let Recorded::Done(answer) = recorder.lock().unwrap().settle(Instant::now()) {
                    return Ok(answer);
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(refusal("record_timeout", "no key was pressed"));
                }
            }
        }
    }
}

/// Why work on the blocking pool did not come back.
#[derive(Debug)]
enum Late {
    Timeout,
    Failed(String),
}

/// Runs `work` on the blocking pool and waits for it until `deadline`. Work that finishes later has its result dropped there, which
/// is how a [`Recording`] that starts late gives the keys back (its `Drop`), so it cannot stay active behind a refused recording.
async fn within<T: Send + 'static>(
    deadline: tokio::time::Instant,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Late> {
    match tokio::time::timeout_at(deadline, tauri::async_runtime::spawn_blocking(work)).await {
        Ok(Ok(done)) => Ok(done),
        Ok(Err(why)) => Err(Late::Failed(why.to_string())),
        Err(_) => Err(Late::Timeout),
    }
}

/// A recording on: the hot keys and the listener are let go of, and keys are captured, until this is dropped.
struct Recording {
    app: AppHandle,
    capture: Option<key_capture::Capture>,
}

impl Recording {
    /// On the blocking pool. `None`: the system could not capture, and nothing was left let go of.
    fn start(app: &AppHandle, heard: key_capture::CaptureSink) -> Option<Recording> {
        pause(app);
        match key_capture::capture(app, heard) {
            Some(capture) => Some(Recording { app: app.clone(), capture: Some(capture) }),
            None => {
                resume(app);
                None
            }
        }
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        let (app, capture) = (self.app.clone(), self.capture.take());
        // Giving the keys back waits for locks and the main thread: not here, which may be an async worker.
        drop(tauri::async_runtime::spawn_blocking(move || {
            drop(capture);
            resume(&app);
        }));
    }
}

/// Tells the page why a key was ignored while recording (`recordHint`).
fn tell_hint(app: &AppHandle, hint: Hint) {
    if let Some(window) = crate::room_window(app) {
        let _ = window.eval(format!(
            "window.__sidevoiceHost && window.__sidevoiceHost.recordHint && window.__sidevoiceHost.recordHint({})",
            json!(hint)
        ));
    }
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

/// A hot key went down or up: a combination's action, or an F-key used as a single key.
fn hot_key(app: &AppHandle, action: Action, edge: Edge) {
    let key = match &app.state::<Shortcuts>().current.lock().unwrap().parsed[action.index()] {
        Some(Parsed::Single { key, .. }) => Some(*key),
        _ => None,
    };
    match key {
        Some(key) => feed_key(app, key, edge),
        None => feed(app, action, edge),
    }
}

/// What a listener heard: a single key's own edges go to its machine, and another key to every pending lone key.
fn key_event(app: &AppHandle, heard: Listened) {
    let targets = shortcuts::route(&app.state::<Shortcuts>().current.lock().unwrap().parsed, heard);
    for (key, edge) in targets {
        feed_key(app, key, edge);
    }
}

fn feed(app: &AppHandle, action: Action, edge: Edge) {
    if edge == Edge::Down {
        // A combination pressed is foreign input to every single key waiting on its own.
        foreign(app, None);
    }
    let effect = {
        let runtime = app.state::<Shortcuts>();
        let mut current = runtime.current.lock().unwrap();
        current.machines[action.index()].as_mut().and_then(|machine| machine.on(edge))
    };
    if let Some(effect) = effect {
        act(app, action, effect);
    }
}

/// An edge of a single key: its machine says what that does (by the way it was pressed), and when the clock decides next.
fn feed_key(app: &AppHandle, key: SingleKey, edge: Edge) {
    if edge == Edge::Down {
        // Another key pressed makes what the others were waiting on not a lone key.
        foreign(app, Some(key));
    }
    let (done, deadline) = {
        let runtime = app.state::<Shortcuts>();
        let mut current = runtime.current.lock().unwrap();
        let Some(slot) = current.keys.iter_mut().find(|slot| slot.key == key) else { return };
        (done_by(&slot.carried, slot.machine.on(edge, Instant::now())), slot.machine.deadline())
    };
    for (action, effect) in done {
        act(app, action, effect);
    }
    if let Some(deadline) = deadline {
        arm(app, key, deadline);
    }
}

/// Foreign input to the single keys other than `pressed` (none: to all): a hold not yet started never starts, a pending tap is dropped.
fn foreign(app: &AppHandle, pressed: Option<SingleKey>) {
    let runtime = app.state::<Shortcuts>();
    let mut current = runtime.current.lock().unwrap();
    let keys: Vec<SingleKey> = current.keys.iter().map(|slot| slot.key).collect();
    let others = match pressed {
        Some(pressed) => shortcuts::others(&keys, pressed),
        None => keys,
    };
    for slot in current.keys.iter_mut().filter(|slot| others.contains(&slot.key)) {
        let _ = slot.machine.on(Edge::Other, Instant::now());
    }
}

/// The actions a machine's pulses are, by the way each was pressed.
fn done_by(carried: &Carried, pulses: Vec<Pulse>) -> Vec<(Action, Effect)> {
    pulses.into_iter().filter_map(|(trigger, effect)| carried.action(trigger).map(|action| (action, effect))).collect()
}

/// Looks at `key`'s machine when its clock is up: a hold that has lasted, a tap no second press followed. Until the machine has
/// nothing more to wait for.
fn arm(app: &AppHandle, key: SingleKey, deadline: Instant) {
    let app = app.clone();
    let _ = std::thread::Builder::new().name("single-key-clock".into()).spawn(move || {
        let mut next = Some(deadline);
        while let Some(at) = next {
            let now = Instant::now();
            let mut watch = Watch::new(now, SystemTime::now());
            if at > now {
                std::thread::sleep(at - now);
            }
            // Woken by a sleep or behind a locked screen, a release was never seen: the key is forgotten, never acted on late
            // (a hold that opened the microphone for a key let go of while the machine slept).
            let looked = watch.look(Instant::now(), SystemTime::now(), session::screen_locked());
            if looked.is_some_and(Release::loses_the_keyboard) {
                forget_key(&app, key);
                return;
            }
            next = poll_key(&app, key);
        }
    });
}

/// Forgets what `key`'s machine is in the middle of.
fn forget_key(app: &AppHandle, key: SingleKey) {
    let runtime = app.state::<Shortcuts>();
    let mut current = runtime.current.lock().unwrap();
    if let Some(slot) = current.keys.iter_mut().find(|slot| slot.key == key) {
        slot.machine.cancel();
    }
}

/// What the clock alone decides for `key` now; when it decides next.
fn poll_key(app: &AppHandle, key: SingleKey) -> Option<Instant> {
    let (done, deadline) = {
        let runtime = app.state::<Shortcuts>();
        let mut current = runtime.current.lock().unwrap();
        let slot = current.keys.iter_mut().find(|slot| slot.key == key)?;
        (done_by(&slot.carried, slot.machine.poll(Instant::now())), slot.machine.deadline())
    };
    for (action, effect) in done {
        act(app, action, effect);
    }
    deadline
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
        for slot in current.keys.iter_mut().filter(|slot| slot.carried.hold.is_some()) {
            slot.machine.cancel();
        }
        current.hold.end()
    };
    if was_on {
        tracing::info!(reason = ?why, "hold to talk let go");
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
    use sidevoice_desktop_core::shortcuts::{Gesture, Trigger, STUCK_HOLD};

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
        assert_eq!(of(Action::HoldToTalk, "Ctrl+Alt+H"), Some(Gesture::Hold));
        assert_eq!(of(Action::Mute, "Ctrl+Alt+M"), Some(Gesture::Press));
        assert_eq!(of(Action::OneTurn, "RightControl:double"), None, "a single key is a KeyMachine's");
        assert_eq!(STUCK_HOLD, Duration::from_secs(60));
    }

    #[test]
    fn a_machines_pulses_are_the_actions_that_carry_each_way() {
        let carried = Carried { hold: Some(Action::HoldToTalk), tap: None, double: Some(Action::OneTurn) };
        let pulses =
            vec![(Trigger::Hold, Effect::HoldStart), (Trigger::Tap, Effect::Fire), (Trigger::Double, Effect::Fire)];
        assert_eq!(
            done_by(&carried, pulses),
            [(Action::HoldToTalk, Effect::HoldStart), (Action::OneTurn, Effect::Fire)],
            "a way nothing carries does nothing"
        );
    }

    /// R6: a start that stalls (the main thread, the system's monitor) is bounded by the same deadline as the wait for a key, and
    /// a start that finishes late drops what it made.
    #[test]
    fn a_start_that_stalls_is_given_up_on_and_what_it_makes_late_is_dropped() {
        #[derive(Debug)]
        struct Dropped(std::sync::mpsc::Sender<()>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let began = Instant::now();
        let answer = tauri::async_runtime::block_on(within(
            tokio::time::Instant::now() + Duration::from_millis(100),
            move || {
                std::thread::sleep(Duration::from_millis(400));
                Dropped(tx)
            },
        ));
        assert!(matches!(answer, Err(Late::Timeout)), "{answer:?}");
        assert!(began.elapsed() < Duration::from_millis(350), "it did not wait for the stalled start");
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok(), "the late start's result is dropped, not kept");
        let quick = tauri::async_runtime::block_on(within(tokio::time::Instant::now() + Duration::from_secs(5), || 7));
        assert!(matches!(quick, Ok(7)));
    }

    #[test]
    fn a_recording_waits_fifteen_seconds() {
        assert_eq!(RECORD_WAIT, Duration::from_secs(15));
    }
}
