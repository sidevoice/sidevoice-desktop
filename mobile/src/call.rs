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
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Whether a voice call has audio open: the session is active.
    audio: AtomicBool,
}

impl PhoneCall {
    /// The system gave the audio back after an interruption (iOS deactivated the session): activates it again if a voice
    /// call has audio open, so that it can record and play on. A session nothing uses stays inactive.
    fn reactivate(&self) {
        if self.audio.load(Ordering::SeqCst) {
            if let Err(error) = Session::activate() {
                log::warn!("call: the audio session was not activated again: {error}");
            }
        }
    }
}

impl CallActivity for PhoneCall {
    fn starting(&self) -> Result<(), String> {
        Session::activate()?;
        self.audio.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn ended(&self) {
        self.audio.store(false, Ordering::SeqCst);
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
        let (on, restore) = match event {
            Interruption::Began => (true, true),
            Interruption::Ended { resume } => {
                phone.reactivate();
                (false, resume)
            }
        };
        if let Some(room) = app.try_state::<RoomCall>() {
            room.then(move |room| room.interrupt(Source::Session, on, restore));
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

/// Whether a change of the audio focus takes the call's audio away (`Some(true)`), gives it back (`Some(false)`) or
/// leaves it (`None`): another app playing quietly over the call (`Duck`) is no interruption.
fn focus_interrupts(state: FocusState) -> Option<bool> {
    match state {
        FocusState::Lost | FocusState::LostTransient => Some(true),
        FocusState::Gained => Some(false),
        FocusState::Duck => None,
    }
}

impl RoomCall {
    /// The room of `app`, for the host's current page `page`.
    pub fn new(app: AppHandle, page: u64) -> RoomCall {
        let (jobs, received) = mpsc::channel::<Job>();
        let mut room = Room {
            app,
            page,
            holding: None,
            starts: 0,
            interrupted: Interrupted::default(),
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
            room.hold(false, None);
        });
    }

    /// The page `page` says whether its call is joined (the green and the red button), and since when (`since`, the
    /// page's own mark of that call: a join of the same page later is another call).
    pub fn report(&self, page: u64, joined: bool, since: Option<f64>) {
        self.then(move |room| {
            if room.page == page {
                room.hold(joined, since);
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
                    log::info!("call: started");
                }
                // The call goes on in front; it does not outlive the screen. Recorded by its code
                // (`not-visible`, `microphone-permission`, …).
                Err(error) => log::error!("call: the call's service did not start: {error}"),
            }
        } else {
            self.holding = None;
            self.clear_interruption();
            if let Err(error) = stop_call_service(&self.app) {
                log::warn!("call: the call's service did not stop: {error}");
            }
            log::info!("call: ended");
        }
    }

    /// `source` is on or off: the call pauses when the first interruption begins and resumes when the last one ends. Nothing
    /// of it while no room call is held (a try in Settings is not paused). `restore_mic`: whether the person's mute is given
    /// back as it was; the system advises against resuming (iOS `shouldResume` false) when another app goes on playing.
    fn interrupt(&mut self, source: Source, on: bool, restore_mic: bool) {
        if self.holding.is_none() {
            return;
        }
        match self.interrupted.set(source, on) {
            Some(Change::Pause) => self.pause(),
            Some(Change::Resume) => self.resume(restore_mic),
            None => {}
        }
    }

    /// The call ends (or its page goes) while interrupted: what the interruption did is given back, so that the next call
    /// does not start muted or silent.
    fn clear_interruption(&mut self) {
        if self.interrupted.any() {
            self.interrupted = Interrupted::default();
            self.resume(true);
        }
    }

    /// The page mutes (its own mute, remembered by the page as ours) and the agent's playback is held.
    fn pause(&self) {
        log::info!("call: paused, the system took the audio");
        if let Some(host) = self.app.try_state::<AppHost>() {
            host.pause_playback(true);
        }
        run_on_page(
            &self.app,
            serde_json::json!({ "command": "interrupt", "page": self.page }),
        );
    }

    /// The page gives back the mute it took (only if the person has not changed it since) and the playback goes on.
    fn resume(&self, restore_mic: bool) {
        log::info!("call: resumed, the system gave the audio back");
        run_on_page(
            &self.app,
            serde_json::json!({ "command": "resume", "page": self.page, "restoreMic": restore_mic }),
        );
        if let Some(host) = self.app.try_state::<AppHost>() {
            host.pause_playback(false);
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
        if let (Some(on), Some(room)) = (focus_interrupts(state), app.try_state::<RoomCall>()) {
            room.then(move |room| {
                if room.holding == Some(start) && room.page == page {
                    room.interrupt(Source::Focus, on, true);
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
        assert_eq!(focus_interrupts(FocusState::Lost), Some(true));
        assert_eq!(focus_interrupts(FocusState::LostTransient), Some(true));
        assert_eq!(focus_interrupts(FocusState::Gained), Some(false));
        assert_eq!(focus_interrupts(FocusState::Duck), None);
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
