//! The call's lifecycle on a phone (D-CALL-LIFECYCLE), the same on Android and iOS: the call starts when the person
//! presses the green call button and ends at the red hang-up button.
//!
//! - **The room's call** (the page reports it joined, `call_state`): the call's foreground service runs for exactly that
//!   (Android; iOS keeps the call alive with its `audio` background mode). It starts when the page says the call is joined
//!   (a press of the green button, so the app is in front, as Android requires), and stops when it says it is not (the
//!   red button, the page reloading, the app quitting). The service ending on its own (the person pressed Hang up in its
//!   notification, removed the app from the recents list, or the system ended it) hangs up the call on the page, as the
//!   red button does.
//! - **The call's audio** ([`PhoneCall`], sidevoice-app-core's `CallActivity`): the audio session is activated before
//!   the first voice call's audio opens and deactivated once the last one's has stopped (iOS; declared at the app's start,
//!   so that opening the app interrupts no other app's audio). Every voice call counts here, a try in Settings too: it plays
//!   and records as well.
//! - **The system taking the audio away** (D-AUDIO-FOCUS: a phone call, an alarm, another app that plays; iOS: the audio
//!   session is interrupted, Android: the call's audio focus is lost): the conversation pauses while it lasts and resumes
//!   when it ends; it never hangs up on its own. Pausing is the page's mute, so that the page shows the call muted, and
//!   the agent's playback held ([`AppHost::pause_playback`]); resuming gives the mute back as it was (a person who had
//!   muted themselves stays muted).
use std::sync::{mpsc, Arc, Mutex};

use crate::platform::interruption::{self, Interruption};
use sidevoice_app_core::app::{AppHost, CallActivity};
use sidevoice_app_core::sidevoice_voice::{AudioSession, Session, SessionConfig, SessionMode};
use sidevoice_mobile_plugin::{
    start_call_service, stop_call_service, CallEvent, FocusState, StopReason,
};
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

/// The phone's side of the host's voice calls: the audio session while any of them has audio open.
#[derive(Default)]
pub struct PhoneCall {
    /// Whether a voice call has audio open: the session is active. Held across every activation and deactivation, so that
    /// the state and the session change together (an interruption's end cannot activate it again after the last call's
    /// audio has stopped).
    audio: Mutex<bool>,
}

impl PhoneCall {
    /// The system gave the audio back after an interruption (iOS deactivated the session): activates it again if a voice
    /// call has audio open, so that it can record and play on. A session nothing uses stays inactive. Whether the audio can
    /// go on: false when the system refused.
    fn reactivate(&self) -> bool {
        let audio = self.audio.lock().unwrap();
        if !*audio {
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
}

impl CallActivity for PhoneCall {
    fn starting(&self) -> Result<(), String> {
        let mut audio = self.audio.lock().unwrap();
        Session::activate()?;
        *audio = true;
        Ok(())
    }

    fn ended(&self) {
        let mut audio = self.audio.lock().unwrap();
        *audio = false;
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
    /// Which of the system's interruptions are on now (only while the room's call is held).
    interrupted: Interrupted,
    /// The call being held, by the page's mark of when it was joined: what the interruption commands are for.
    since: Option<f64>,
    /// After an interruption that does not end by itself (the system advised against resuming, the audio could not be taken
    /// back, or the focus was refused) the microphone stays muted and the playback held until the person unmutes. See
    /// [`Wait`].
    waiting: Option<Wait>,
    /// Whether the playback is held.
    paused: bool,
    /// The microphone as the page last reported it (`enabled`, `disabled`).
    mic: (bool, bool),
    /// The audio session, given back before the playback goes on after an interruption that did not end by itself.
    phone: Arc<PhoneCall>,
}

/// Where an interruption comes from: the call can be taken away by both at once (an alarm on an audio focus that is
/// also lost), and resumes only when neither holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// iOS: the audio session was interrupted.
    Session,
    /// Android: the call's audio focus was lost.
    Focus,
}

/// What an interruption changes of the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Pause,
    Resume,
}

/// Which interruptions are on.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Interrupted {
    session: bool,
    focus: bool,
}

impl Interrupted {
    fn any(self) -> bool {
        self.session || self.focus
    }

    /// `source` is on or off now: what that changes of the call, if anything.
    fn set(&mut self, source: Source, on: bool) -> Option<Change> {
        let was = self.any();
        match source {
            Source::Session => self.session = on,
            Source::Focus => self.focus = on,
        }
        match (was, self.any()) {
            (false, true) => Some(Change::Pause),
            (true, false) => Some(Change::Resume),
            _ => None,
        }
    }
}

/// What the system said, as the call takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Signal {
    /// The audio was taken away.
    Began,
    /// It is back. `resume`: the call goes on by itself (the system advises it, and the audio could be taken back).
    Ended { resume: bool },
    /// The audio focus was refused outright: there is no end to wait for.
    Refused,
}

/// What a change of the audio focus says of the call: another app playing quietly over it (`Duck`) says nothing.
fn focus_signal(state: FocusState) -> Option<Signal> {
    match state {
        FocusState::Lost | FocusState::LostTransient => Some(Signal::Began),
        FocusState::Gained => Some(Signal::Ended { resume: true }),
        FocusState::Refused => Some(Signal::Refused),
        FocusState::Duck => None,
    }
}

/// Where the person's unmute is waited for after an interruption that does not end by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Wait {
    /// Whether the microphone has been seen muted: a report of the page from before the mute reached it is not an unmute.
    muted: bool,
}

impl Wait {
    /// The wait that begins with the microphone as the page last reported it (`enabled`, `disabled`): muted already if the
    /// page said so, whether before the interruption (the person's own mute) or since it began.
    fn of(mic: (bool, bool)) -> Wait {
        Wait { muted: !mic.0 }
    }
}

/// The page reports the microphone (`enabled`, `disabled`: the mute cannot be used): the wait after it, and whether the
/// person has lifted the pause. A microphone that cannot be muted has nothing to wait for.
fn wait_after(wait: Wait, enabled: bool, disabled: bool) -> (Option<Wait>, bool) {
    if disabled {
        return (None, true);
    }
    match (wait.muted, enabled) {
        (false, true) => (Some(wait), false),
        (false, false) | (true, false) => (Some(Wait { muted: true }), false),
        (true, true) => (None, true),
    }
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
            interrupted: Interrupted::default(),
            since: None,
            waiting: None,
            paused: false,
            mic: (true, false),
            phone,
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
            room.mic = (true, false);
            room.hold(false, None);
        });
    }

    /// The page `page` says whether its call is joined (the green and the red button), and since when (`since`, the
    /// page's own mark of that call: a join of the same page later is another call).
    /// `mic` is the microphone as the page reports it (`enabled`, `disabled`).
    pub fn report(&self, page: u64, joined: bool, since: Option<f64>, mic: (bool, bool)) {
        self.then(move |room| {
            if room.page == page {
                room.hold(joined, since);
                room.microphone(mic.0, mic.1);
            }
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
            match start_call_service(&self.app, move |event| {
                on_service_event(&app, start, page, since, event)
            }) {
                Ok(()) => {
                    self.holding = Some(start);
                    self.since = since;
                    log::info!("call: started");
                }
                // The call goes on in front; it does not outlive the screen. Recorded by its code
                // (`not-visible`, `microphone-permission`, …).
                Err(error) => log::error!("call: the call's service did not start: {error}"),
            }
        } else {
            self.holding = None;
            self.clear_interruption();
            self.since = None;
            if let Err(error) = stop_call_service(&self.app) {
                log::warn!("call: the call's service did not stop: {error}");
            }
            log::info!("call: ended");
        }
    }

    /// What the system said, from `source`: the call pauses when the first interruption begins and resumes when the last
    /// one ends. Nothing of it while no room call is held (a try in Settings is not paused).
    fn signal(&mut self, source: Source, signal: Signal) {
        if self.holding.is_none() {
            return;
        }
        match signal {
            Signal::Began => {
                if self.interrupted.set(source, true) == Some(Change::Pause) {
                    self.waiting = None;
                    self.pause();
                }
            }
            Signal::Ended { resume } => {
                if self.interrupted.set(source, false) == Some(Change::Resume) {
                    self.resume(resume);
                }
            }
            Signal::Refused => {
                // Nothing will end it: paused, until the person unmutes.
                if !self.interrupted.any() {
                    self.pause();
                    self.waiting = Some(Wait::of(self.mic));
                }
            }
        }
    }

    /// The page reports the microphone: after an interruption that does not end by itself, the person's unmute lifts the
    /// pause (the playback goes on).
    fn microphone(&mut self, enabled: bool, disabled: bool) {
        // Always kept, interrupted or not: a mute reported before the wait begins is the evidence the wait needs (the page
        // reports a change only once).
        self.mic = (enabled, disabled);
        let Some(wait) = self.waiting else { return };
        let (waiting, lifted) = wait_after(wait, enabled, disabled);
        self.waiting = waiting;
        if !lifted {
            return;
        }
        // The audio session may have been refused when the interruption ended: it is asked for again before the playback
        // goes on, and the wait goes on if it is refused again.
        if self.phone.reactivate() {
            log::info!("call: resumed by the person");
            self.hold_playback(false);
        } else {
            self.waiting = Some(Wait { muted: false });
        }
    }

    /// The call ends (or its page goes) while interrupted: the playback is let go, so that the next call is not held. The
    /// page gives the mute back itself when its call ends.
    fn clear_interruption(&mut self) {
        self.interrupted = Interrupted::default();
        if self.waiting.take().is_some() || self.paused {
            self.hold_playback(false);
        }
    }

    fn hold_playback(&mut self, held: bool) {
        self.paused = held;
        if let Some(host) = self.app.try_state::<AppHost>() {
            host.pause_playback(held);
        }
    }

    /// The page mutes (its own mute, remembered by the page as ours) and the agent's playback is held.
    fn pause(&mut self) {
        log::info!("call: paused, the system took the audio");
        self.hold_playback(true);
        run_on_page(
            &self.app,
            serde_json::json!({ "command": "interrupt", "page": self.page, "since": self.since }),
        );
    }

    /// The audio is back. With `resume` the page gives back the mute it took (only if the person has not changed it since)
    /// and the playback goes on; without it (the system advises against, or the audio could not be taken back) both stay
    /// until the person unmutes.
    fn resume(&mut self, resume: bool) {
        log::info!("call: the system gave the audio back (resume: {resume})");
        run_on_page(
            &self.app,
            serde_json::json!({ "command": "resume", "page": self.page, "since": self.since, "restoreMic": resume }),
        );
        if resume {
            self.hold_playback(false);
        } else {
            self.waiting = Some(Wait::of(self.mic));
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
    if let CallEvent::Focus { state } = event {
        if let (Some(signal), Some(room)) = (focus_signal(state), app.try_state::<RoomCall>()) {
            room.then(move |room| {
                if room.holding == Some(start) && room.page == page {
                    room.signal(Source::Focus, signal);
                }
            });
        }
        return;
    }
    if let Some(reason) = hang_up_for(&event) {
        let Some(room) = app.try_state::<RoomCall>() else {
            return;
        };
        room.then(move |room| {
            if room.holding != Some(start) || room.page != page {
                return; // an older service's end: the call it was for is over already
            }
            log::info!("call: hung up by the service ({reason:?})");
            // The service is gone: the next join starts another.
            room.holding = None;
            room.clear_interruption();
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
        }
        | CallEvent::Focus { .. } => None,
        CallEvent::Stopped { reason } => Some(*reason),
    }
}

/// Runs one of the shared commands on the room through the page's `run`: `request` is `{command, …}` (`hang-up`,
/// `toggle-mute`, …), and with `page` and `since` it runs only on that page's call joined then (mobile/js/mobile-bridge.js).
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
        assert_eq!(
            hang_up_for(&CallEvent::Focus {
                state: FocusState::Lost
            }),
            None
        );
    }

    #[test]
    fn the_focus_pauses_the_call_and_gives_it_back_and_never_hangs_up() {
        assert_eq!(focus_signal(FocusState::Lost), Some(Signal::Began));
        assert_eq!(focus_signal(FocusState::LostTransient), Some(Signal::Began));
        assert_eq!(
            focus_signal(FocusState::Gained),
            Some(Signal::Ended { resume: true })
        );
        assert_eq!(focus_signal(FocusState::Refused), Some(Signal::Refused));
        assert_eq!(focus_signal(FocusState::Duck), None);
    }

    #[test]
    fn a_pause_without_an_end_waits_for_the_person_to_unmute() {
        let waiting = Wait { muted: false };
        let muted = Wait { muted: true };
        // A report from before the mute reached the page is not an unmute.
        assert_eq!(wait_after(waiting, true, false), (Some(waiting), false));
        assert_eq!(wait_after(waiting, false, false), (Some(muted), false));
        assert_eq!(wait_after(muted, false, false), (Some(muted), false));
        assert_eq!(wait_after(muted, true, false), (None, true));
        // A microphone that cannot be muted has nothing to wait for.
        assert_eq!(wait_after(waiting, true, true), (None, true));
    }

    #[test]
    fn a_mute_reported_before_the_wait_begins_counts() {
        // Muted (by the interruption, or by the person before it), reported once; then the wait begins; then they unmute.
        let wait = Wait::of((false, false));
        assert_eq!(wait, Wait { muted: true });
        assert_eq!(wait_after(wait, true, false), (None, true));
        // Not yet muted when it begins: the unmute must come after the mute.
        let wait = Wait::of((true, false));
        assert_eq!(wait_after(wait, true, false), (Some(wait), false));
        let (wait, lifted) = wait_after(wait, false, false);
        assert!(!lifted);
        assert_eq!(wait_after(wait.unwrap(), true, false), (None, true));
    }

    #[test]
    fn the_call_pauses_at_the_first_interruption_and_resumes_at_the_last_end() {
        let mut now = Interrupted::default();
        assert_eq!(now.set(Source::Focus, false), None, "nothing was on");
        assert_eq!(now.set(Source::Focus, true), Some(Change::Pause));
        assert_eq!(now.set(Source::Focus, true), None, "already paused");
        assert_eq!(
            now.set(Source::Session, true),
            None,
            "paused by the other already"
        );
        assert_eq!(
            now.set(Source::Focus, false),
            None,
            "the session still holds it"
        );
        assert_eq!(now.set(Source::Session, false), Some(Change::Resume));
        assert_eq!(now, Interrupted::default());
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
