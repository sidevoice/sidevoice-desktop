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

use crate::hold::{focus_signal, Effect, Hold, Signal, Source};
use crate::platform::interruption::{self, Interruption};
use sidevoice_app_core::app::{AppHost, CallActivity};
use sidevoice_app_core::sidevoice_voice::{AudioSession, Session, SessionConfig, SessionMode};
use sidevoice_mobile_plugin::{
    request_call_focus, start_call_service, stop_call_service, CallEvent, StopReason,
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
    /// The call being held, by the page's mark of when it was joined: what the interruption commands are for.
    since: Option<f64>,
    /// What the system's interruptions have done to the call (`hold.rs`).
    interruptions: Hold,
    /// The audio session, given back before the playback goes on after an interruption that did not end by itself.
    phone: Arc<PhoneCall>,
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
            room.interruptions = Hold::new();
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
