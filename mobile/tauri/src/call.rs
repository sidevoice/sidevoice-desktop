//! The call's lifecycle on a phone (D-CALL-LIFECYCLE), the same on Android and iOS: the call starts when the person
//! presses the green call button and ends at the red hang-up button.
//!
//! - **The room's call** (the page reports it joined, `call_state`) is a phone call of the system's own for exactly that
//!   long (the plugin's call service: a CallKit call on iOS; on Android the call's foreground service and the self-managed
//!   Telecom call it holds). It starts when the page says the call is joined (a press of the green button, so the app is in
//!   front, as Android requires), and ends when it says it is not (the red button, the page reloading, the app quitting).
//!   The system's call ending on its own (the person hung up from the system's screens, the notification, a headset or a
//!   car kit, removed the app from the recents list, or the system ended it) hangs up the call on the page, as the red
//!   button does. The system shows the conversation the call is on and the page's mute; a mute from the system's side
//!   (its screens, a Bluetooth or car kit) is the page's mute too, as the person's own.
//! - **The call's audio** ([`PhoneCall`], sidevoice-shared's `CallActivity`): the audio session is active from before the
//!   first voice call's audio opens until the last one's has stopped (iOS; declared at the app's start, so that opening the
//!   app interrupts no other app's audio). Every voice call counts here, a try in Settings too: it plays and records as
//!   well. While the room's call is a CallKit call, CallKit activates and deactivates the session (Apple's rule for a
//!   CallKit call): the first voice call's audio waits for it to say so.
//! - **The system taking the audio away** (D-AUDIO-FOCUS: a phone call, an alarm, another app that plays; iOS: the audio
//!   session is interrupted, Android: the call's audio focus is lost; and the system's call put on hold, CallKit's or
//!   Telecom's): the conversation pauses while it lasts and resumes when it ends; it never hangs up on its own. Pausing is the page's mute, so that the page shows the call muted, and
//!   the agent's playback held ([`AppHost::pause_playback`]); resuming gives the mute back as it was (a person who had
//!   muted themselves stays muted).
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::hold::{focus_signal, Effect, Hold, Signal, Source};
use crate::platform::interruption::{self, Interruption};
use sidevoice_mobile_plugin::{
    call_activates_audio, call_replaces_media_controls, request_call_focus, start_call_service,
    stop_call_service, update_call_service, CallDetails, CallEvent, StopReason,
};
use sidevoice_shared::app::{AppHost, CallActivity};
use sidevoice_shared::sidevoice_voice::{AudioSession, Session, SessionConfig, SessionMode};
use sidevoice_shared_tauri::platform::media_controls::{self, CallState};
use tauri::{AppHandle, Manager};

/// How the call uses the audio session (iOS; D-AUDIO-SESSION, sidevoice-voice's `SessionConfig`): play-and-record, the
/// system's own processing (`Default`: the voice's echo cancellation is the app's), the loudspeaker rather than the
/// receiver when nothing else is plugged in, and a Bluetooth hands-free headset allowed.
const CALL_AUDIO: SessionConfig = SessionConfig {
    mode: SessionMode::Default,
    speaker: true,
    bluetooth: true,
};

/// Declares the call's use of the audio session, at the app's start. Activates nothing.
pub fn configure() -> Result<(), String> {
    Session::configure(&CALL_AUDIO)
}

/// How long the first voice call's audio waits for the system to activate the session of its call (CallKit answers in well
/// under a second). Past it the voice call is refused (`audio-session-activate-failed`): the session is CallKit's to
/// activate while its call lasts, never the app's.
const SYSTEM_AUDIO_LIMIT: Duration = Duration::from_secs(5);

/// The phone's side of the host's voice calls: the audio session while any of them has audio open.
#[derive(Default)]
pub struct PhoneCall {
    /// Held across every activation and deactivation, so that the state and the session change together (an interruption's
    /// end cannot activate it again after the last call's audio has stopped).
    audio: Mutex<Audio>,
    /// Told whenever the system's call or its audio changes: the first voice call's audio may be waiting for it.
    changed: Condvar,
}

#[derive(Debug, Default)]
struct Audio {
    /// Whether a voice call has audio open: the session is active.
    open: bool,
    /// The system's call, where the system activates its audio session ([`call_activates_audio`]).
    system: SystemAudio,
}

/// The audio session of the system's call, where the system activates it (CallKit).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum SystemAudio {
    /// No such call: the app activates the session itself.
    #[default]
    None,
    /// A call is starting: the system activates the session when it is ready.
    Pending,
    /// The system activated the session and has deactivated it since (the call is on hold, another call has the audio).
    Held,
    /// The system has activated it.
    Active,
}

/// What a voice call's audio refused for want of the system's session says (sidevoice-voice's code for it).
const ACTIVATE_FAILED: &str = "audio-session-activate-failed";

impl PhoneCall {
    /// The system gave the audio back after an interruption (iOS deactivated the session): activates it again if a voice
    /// call has audio open, so that it can record and play on. A session nothing uses stays inactive, and the system's call
    /// gets its session back from the system. Whether the audio can go on: false when the system refused.
    fn reactivate(&self) -> bool {
        let audio = self.audio.lock().unwrap();
        if !audio.open || audio.system != SystemAudio::None {
            return true;
        }
        match Session::activate() {
            Ok(()) => true,
            Err(error) => {
                log::warn!("call: the audio session was not activated again: {error}");
                false
            }
        }
    }

    /// The room's call is a call of the system's that activates its own audio session (`on`), or is not any more.
    fn system_call(&self, on: bool) {
        let mut audio = self.audio.lock().unwrap();
        audio.system = match (on, audio.system) {
            (false, _) => SystemAudio::None,
            (true, SystemAudio::None) => SystemAudio::Pending,
            (true, now) => now,
        };
        self.changed.notify_all();
    }

    /// The system activated (`active`) or deactivated the audio session of its call.
    fn system_audio(&self, active: bool) {
        let mut audio = self.audio.lock().unwrap();
        if audio.system != SystemAudio::None {
            audio.system = if active {
                SystemAudio::Active
            } else {
                SystemAudio::Held
            };
        }
        self.changed.notify_all();
    }
}

impl CallActivity for PhoneCall {
    /// Before the first voice call's audio opens: the session is activated, by the app, or (during a call of the system's
    /// that activates its own) by the system, waited for while the call starts. While the system holds its call, or past the
    /// wait, the voice call is refused: the app never activates a session the system owns.
    fn starting(&self) -> Result<(), String> {
        let mut audio = self.audio.lock().unwrap();
        let deadline = Instant::now() + SYSTEM_AUDIO_LIMIT;
        loop {
            match audio.system {
                SystemAudio::None => {
                    Session::activate()?;
                    break;
                }
                SystemAudio::Active => break,
                SystemAudio::Held => {
                    log::warn!("call: the system's call is on hold; no audio until it is back");
                    return Err(ACTIVATE_FAILED.into());
                }
                SystemAudio::Pending => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        log::warn!(
                            "call: the system did not activate its call's audio session in time"
                        );
                        return Err(ACTIVATE_FAILED.into());
                    }
                    audio = self.changed.wait_timeout(audio, left).unwrap().0;
                }
            }
        }
        audio.open = true;
        Ok(())
    }

    /// The last voice call's audio has stopped: the session is given back, unless the system's call owns it (the system
    /// deactivates it when the call ends).
    fn ended(&self) {
        let mut audio = self.audio.lock().unwrap();
        audio.open = false;
        if audio.system != SystemAudio::None {
            return;
        }
        if let Err(error) = Session::deactivate() {
            log::warn!("call: the audio session was not given back: {error}");
        }
    }
}

/// Pauses the room's call while the system has the audio session interrupted (iOS) and resumes it when that ends, and
/// gives `phone` the session back first so that the call's audio can go on (D-AUDIO-FOCUS). Android needs none of this
/// (the call's service reports its audio focus: [`on_service_event`]).
pub fn watch_interruptions(app: &AppHandle, phone: Arc<PhoneCall>) {
    let app = app.clone();
    let listener = Arc::new(move |event: Interruption| {
        log::info!("audio session: {event:?}");
        let signal = match event {
            Interruption::Began => Signal::Began,
            // Not resumed on its own when the system advises against it, or the session could not be activated again.
            Interruption::Ended { resume } => Signal::Ended {
                resume: phone.reactivate() && resume,
            },
        };
        if let Some(room) = app.try_state::<RoomCall>() {
            room.then(move |room| room.signal(Source::Session, signal));
        }
    });
    if let Err(error) = interruption::observe(listener) {
        log::error!("audio session: {error}");
    }
}

/// The room's call and the call's service, in one place: every change (a report of the page, a new page, the service
/// ending on its own) is a job, done in order by one thread of their own, which is the only one to touch the service.
/// So no lock is held while the plugin is waited for (it answers from Android's UI thread, which also hands the
/// service's reports over), a callback of the service never waits, and a change made for a page that has since been
/// replaced does nothing.
pub struct RoomCall {
    jobs: Mutex<mpsc::Sender<Job>>,
}

type Job = Box<dyn FnOnce(&mut Room) + Send>;

/// What only the room's thread holds.
pub struct Room {
    pub app: AppHandle,
    /// The page whose call this is (the host's page generation): reports of another are not its.
    pub page: u64,
    /// The service that runs for it, by the number of its start (`None`: none runs). Each start has its own number, so a
    /// report of a service that has since been replaced is not taken for the new one's.
    holding: Option<u64>,
    /// The number of the last start.
    starts: u64,
    /// The call being held, by the page's mark of when it was joined: what the interruption commands are for.
    since: Option<f64>,
    /// What the system's interruptions have done to the call (`hold.rs`).
    interruptions: Hold,
    /// The audio session, given back before the playback goes on after an interruption that did not end by itself.
    phone: Arc<PhoneCall>,
    /// The call as the page last reported it.
    state: CallState,
    /// What the system's call was last told it shows (`None`: nothing yet).
    shown: Option<CallDetails>,
}

impl RoomCall {
    /// The room of `app`, for the host's current page `page`.
    pub fn new(app: AppHandle, page: u64, phone: Arc<PhoneCall>) -> RoomCall {
        let (jobs, received) = mpsc::channel::<Job>();
        let mut room = Room {
            app,
            page,
            holding: None,
            starts: 0,
            since: None,
            interruptions: Hold::new(),
            phone,
            state: CallState::default(),
            shown: None,
        };
        std::thread::Builder::new()
            .name("room-call".into())
            .spawn(move || {
                for job in received {
                    job(&mut room);
                }
            })
            .expect("a thread for the room's call");
        RoomCall {
            jobs: Mutex::new(jobs),
        }
    }

    /// Queues `job`; it runs after every job queued before it.
    pub fn then(&self, job: impl FnOnce(&mut Room) + Send + 'static) {
        let _ = self.jobs.lock().unwrap().send(Box::new(job));
    }

    /// A page loaded: the call of the one before ends, and only the new page's reports count from now on. Queued at once,
    /// in order with what the new page reports after it.
    pub fn page_changed(&self, page: u64) {
        self.then(move |room| {
            room.page = page;
            room.state = CallState::default();
            room.hold(false, None);
            // What the old page's interruptions held is let go first; the new page starts with none.
            room.clear_interruption();
            room.interruptions = Hold::new();
            // The media controls let go too.
            room.media();
        });
    }

    /// The page `page` reports its call: whether it is joined (the green and the red button), since when (`since`, the
    /// page's own mark of that call: a join of the same page later is another call), its conversation and its mute
    /// (`state`), and whether the mute can be used (`mic_disabled`).
    pub fn report(&self, page: u64, state: CallState, since: Option<f64>, mic_disabled: bool) {
        self.then(move |room| {
            if room.page == page {
                let (joined, mic_enabled) = (state.joined, state.mic_enabled);
                room.state = state;
                room.hold(joined, since);
                room.microphone(mic_enabled, mic_disabled);
                room.show();
                room.media();
            }
        });
    }

    /// A headset's button or the lock screen asked to mute or unmute: the page's own mute, as the interruptions use it, for
    /// the current page's call (its number, and the call's join mark when its service holds it), in order with the rest.
    pub fn toggle_mute(&self) {
        self.then(|room| {
            let mut request = serde_json::json!({ "command": "toggle-mute", "page": room.page });
            if let Some(since) = room.since {
                request["since"] = since.into();
            }
            run_on_page(&room.app, request);
        });
    }

    /// Quitting: the service stops, waited for up to `limit`.
    pub fn end(&self, limit: std::time::Duration) {
        let (done, finished) = mpsc::channel();
        self.then(move |room| {
            room.hold(false, None);
            let _ = done.send(());
        });
        let _ = finished.recv_timeout(limit);
    }
}

impl Room {
    /// Starts or stops the call's service, as `joined` says, for the call joined at `since`.
    fn hold(&mut self, joined: bool, since: Option<f64>) {
        if joined == self.holding.is_some() {
            return;
        }
        if joined {
            self.starts += 1;
            let (app, start, page) = (self.app.clone(), self.starts, self.page);
            let details = CallDetails {
                title: self.state.title.trim().to_owned(),
                muted: false,
            };
            // A voice call's audio that opens meanwhile waits for the system to activate the session (CallKit).
            let system_audio = call_activates_audio();
            self.phone.system_call(system_audio);
            match start_call_service(&self.app, &details, move |event| {
                on_service_event(&app, start, page, since, event)
            }) {
                Ok(()) => {
                    self.holding = Some(start);
                    self.since = since;
                    // The system is told the page's mute and title at once (`show`), whatever it started with.
                    self.shown = None;
                    log::info!("call: started");
                }
                // The call goes on as the app's own (in front on Android: it does not outlive the screen; on iOS with the
                // app's own audio session and media controls). Recorded by its code (`not-visible`,
                // `microphone-permission`, `call-refused`, …).
                Err(error) => {
                    self.phone.system_call(false);
                    log::error!("call: the system's call did not start: {error}");
                }
            }
        } else {
            self.holding = None;
            self.clear_interruption();
            self.since = None;
            self.shown = None;
            self.phone.system_call(false);
            if let Err(error) = stop_call_service(&self.app) {
                log::warn!("call: the call's service did not stop: {error}");
            }
            log::info!("call: ended");
        }
    }

    /// Tells the system's call what it shows now, when that changed: the page's conversation and its mute.
    fn show(&mut self) {
        if self.holding.is_none() {
            return;
        }
        let details = CallDetails {
            title: self.state.title.trim().to_owned(),
            muted: !self.state.mic_enabled,
        };
        if self.shown.as_ref() == Some(&details) {
            return;
        }
        match update_call_service(&self.app, &details) {
            Ok(()) => self.shown = Some(details),
            Err(error) => log::warn!("call: the system's call was not updated: {error}"),
        }
    }

    /// The media controls follow the page's call (the media controls reach the plugin's Kotlin, which runs on Android's
    /// UI thread: never from there). Where the system's call takes their place, they show none while it lasts.
    fn media(&self) {
        let state = if self.holding.is_some() && call_replaces_media_controls() {
            CallState {
                ready: self.state.ready,
                ..CallState::default()
            }
        } else {
            self.state.clone()
        };
        media_controls::update(&self.app, &state);
    }

    /// What the system said, from `source`. Nothing of it while no room call is held (a try in Settings is not paused).
    fn signal(&mut self, source: Source, signal: Signal) {
        if self.holding.is_none() {
            return;
        }
        let effects = self.interruptions.signal(source, signal);
        self.apply(effects);
    }

    /// The page reports the microphone: after an interruption that does not end by itself, the person's unmute lifts the
    /// pause.
    fn microphone(&mut self, enabled: bool, disabled: bool) {
        let (app, phone) = (self.app.clone(), self.phone.clone());
        let effects = self.interruptions.microphone(
            enabled,
            disabled,
            || phone.reactivate(),
            || match request_call_focus(&app) {
                Ok(()) => true,
                Err(error) => {
                    log::warn!("call: the audio focus was not asked for again: {error}");
                    false
                }
            },
        );
        self.apply(effects);
    }

    /// The call ends (or its page goes) while interrupted: the playback is let go, so that the next call is not held. The
    /// page gives the mute back itself when its call ends.
    fn clear_interruption(&mut self) {
        let effects = self.interruptions.clear();
        self.apply(effects);
    }

    fn apply(&self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                // The page mutes (its own mute, remembered by the page as ours).
                Effect::Interrupt => {
                    log::info!("call: paused, the system took the audio");
                    run_on_page(
                        &self.app,
                        serde_json::json!({ "command": "interrupt", "page": self.page, "since": self.since }),
                    );
                }
                // The page gives back the mute it took, if it is still the system's.
                Effect::Resume { restore_mic } => run_on_page(
                    &self.app,
                    serde_json::json!({ "command": "resume", "page": self.page, "since": self.since, "restoreMic": restore_mic }),
                ),
                Effect::Playback(held) => {
                    log::info!(
                        "call: the playback is {}",
                        if held { "held" } else { "released" }
                    );
                    if let Some(host) = self.app.try_state::<AppHost>() {
                        host.pause_playback(held);
                    }
                }
            }
        }
    }
}

/// What the service of start `start`, made for the call of page `page` joined at `since`, reports, as it reports it (it
/// must not wait): an end the app did not ask for hangs that call up, on the room's thread and only while that service
/// is still the one that runs for that page, and on the page only if that call is still the one it has (the page checks
/// `page` and `since` when the command reaches it, which can be after another page or call has taken its place); a
/// change of the audio focus pauses the call or resumes it (an interruption never hangs up).
fn on_service_event(app: &AppHandle, start: u64, page: u64, since: Option<f64>, event: CallEvent) {
    log::info!("call service: {event:?}");
    let Some(room) = app.try_state::<RoomCall>() else {
        return;
    };
    // What only the call of this start, on this page, acts on.
    let mine = move |room: &Room| room.holding == Some(start) && room.page == page;
    match event {
        CallEvent::Focus { state } => {
            if let Some(signal) = focus_signal(state) {
                room.then(move |room| {
                    if mine(room) {
                        room.signal(Source::Focus, signal);
                    }
                });
            }
            return;
        }
        // On hold: paused as an interruption, resumed when taken off it.
        CallEvent::Held { held } => {
            room.then(move |room| {
                if mine(room) {
                    let signal = if held {
                        Signal::Began
                    } else {
                        Signal::Ended { resume: true }
                    };
                    room.signal(Source::System, signal);
                }
            });
            return;
        }
        // Muted or unmuted from the system's side: the page's mute, set to that (nothing if it is so already).
        CallEvent::Muted { muted } => {
            room.then(move |room| {
                if mine(room) {
                    if let Some(shown) = room.shown.as_mut() {
                        shown.muted = muted;
                    }
                    run_on_page(
                        &room.app,
                        serde_json::json!({ "command": "mute", "page": page, "since": since, "muted": muted }),
                    );
                }
            });
            return;
        }
        // The system activated or deactivated its call's audio session: a voice call's audio may be waiting for it.
        CallEvent::Audio { active } => {
            room.then(move |room| {
                if mine(room) {
                    room.phone.system_audio(active);
                    // The system gives a CallKit call its session back by activating it, and may not end the interruption
                    // it posted for the session: an activation is that end (nothing, when the session was not interrupted).
                    if active {
                        room.signal(Source::Session, Signal::Ended { resume: true });
                    }
                }
            });
            return;
        }
        CallEvent::Stopped { .. } => {}
    }
    if let Some(reason) = hang_up_for(&event) {
        room.then(move |room| {
            if !mine(room) {
                return; // an older service's end: the call it was for is over already
            }
            log::info!("call: hung up by the service ({reason:?})");
            // The service is gone: the next join starts another.
            room.holding = None;
            room.shown = None;
            room.phone.system_call(false);
            room.clear_interruption();
            room.media();
            run_on_page(
                &room.app,
                serde_json::json!({ "command": "hang-up", "page": page, "since": since }),
            );
        });
    }
}

/// Whether `event` ends the call: the service stopped other than at the app's own request.
fn hang_up_for(event: &CallEvent) -> Option<StopReason> {
    match event {
        CallEvent::Stopped {
            reason: StopReason::Requested,
        } => None,
        CallEvent::Stopped { reason } => Some(*reason),
        CallEvent::Focus { .. }
        | CallEvent::Muted { .. }
        | CallEvent::Held { .. }
        | CallEvent::Audio { .. } => None,
    }
}

/// Runs one of the shared commands on the room through the page's `run`: `request` is `{command, …}` (`hang-up`,
/// `toggle-mute`, …), and with `page` and `since` it runs only on that page's call joined then (mobile/tauri/js/mobile-bridge.js).
pub fn run_on_page(app: &AppHandle, request: serde_json::Value) {
    if let Some(window) = app.get_webview_window(crate::WINDOW) {
        let _ = window.eval(format!(
            "window.__sidevoiceHost && window.__sidevoiceHost.run({request});"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidevoice_mobile_plugin::FocusState;

    #[test]
    fn only_an_end_the_app_did_not_ask_for_hangs_up() {
        for reason in [
            StopReason::HangUp,
            StopReason::TaskRemoved,
            StopReason::System,
        ] {
            assert_eq!(hang_up_for(&CallEvent::Stopped { reason }), Some(reason));
        }
        assert_eq!(
            hang_up_for(&CallEvent::Stopped {
                reason: StopReason::Requested
            }),
            None
        );
        for event in [
            CallEvent::Focus {
                state: FocusState::Lost,
            },
            CallEvent::Muted { muted: true },
            CallEvent::Held { held: true },
            CallEvent::Audio { active: false },
        ] {
            assert_eq!(hang_up_for(&event), None, "{event:?}");
        }
    }

    #[test]
    fn the_session_is_play_and_record_on_the_loudspeaker_with_bluetooth() {
        assert_eq!(
            CALL_AUDIO,
            SessionConfig {
                mode: SessionMode::Default,
                speaker: true,
                bluetooth: true
            }
        );
    }
}
