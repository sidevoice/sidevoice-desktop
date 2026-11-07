//! The call's lifecycle on a phone (D-CALL-LIFECYCLE), the same on Android and iOS: the call starts when the person
//! presses the green call button and ends at the red hang-up button.
//!
//! - **The room's call** (the page reports it joined, `call_state`) is the app's own call for exactly that long (on Android
//!   the call's foreground service; on iOS nothing but the audio session), and a phone call of the system's own only while
//!   the nightly developer option "Treat calls as system calls" is on (D-NO-CALLKIT-DEFAULT, `developer_options.rs`: a
//!   CallKit call on iOS; on Android the service's self-managed Telecom call). It starts when the page says the call is joined (a press
//!   of the green button, so the app is in front, as Android requires), and ends when it says it is not (the red button, the page reloading, the app quitting).
//!   The system's call ending on its own (the person hung up from the system's screens, the notification, a headset or a
//!   car kit, removed the app from the recents list, or the system ended it) hangs up the call on the page, as the red
//!   button does. The system shows the conversation the call is on and the page's mute; a mute from the system's side
//!   (its screens, a Bluetooth or car kit) is the page's mute too, as the person's own.
//! - **The call's audio** ([`PhoneCall`], sidevoice-shared's `CallActivity`): the audio session is active from before the
//!   first voice call's audio opens until the last one's has stopped (iOS; declared at the app's start, so that opening the
//!   app interrupts no other app's audio). Every voice call counts here, a try in Settings too: it plays and records as
//!   well. While the room's call is a system's call, the first voice call's audio waits for the system to make the call's
//!   audio ready: on iOS CallKit activates and deactivates the session (Apple's rule for a CallKit call); on Android
//!   Telecom makes the call active and settles its route (the communication mode and the loudspeaker), which would cut
//!   off any stream opened before (an AAudio disconnect at the call's start).
//! - **The system taking the audio away** (D-AUDIO-FOCUS: a phone call, an alarm, another app that plays; iOS: the audio
//!   session is interrupted, Android: the call's audio focus is lost; and the system's call put on hold, CallKit's or
//!   Telecom's): the conversation pauses while it lasts and resumes when it ends; it never hangs up on its own. Pausing is the page's mute, so that the page shows the call muted, and
//!   the agent's playback held ([`AppHost::pause_playback`]); resuming gives the mute back as it was (a person who had
//!   muted themselves stays muted).
use std::collections::BTreeSet;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::hold::{focus_signal, Effect, Hold, Signal, Source};
use crate::platform::interruption::{self, Interruption};
use sidevoice_mobile_plugin::{
    call_activates_audio, call_replaces_media_controls, clear_stills, forget_stills,
    refresh_call_service, request_call_focus, save_still, start_call_service, stop_call_service,
    update_call_service, CallDetails, CallEvent, CallSurface, StopReason, SurfaceAction,
};
use sidevoice_shared::app::{AppHost, CallActivity};
use sidevoice_shared::sidevoice_voice::{AudioSession, Session, SessionConfig, SessionMode};
use sidevoice_shared_tauri::platform::media_controls::{self, CallState};
use tauri::{AppHandle, Manager};

/// How the call uses the audio session (iOS; D-AUDIO-SESSION, sidevoice-voice's `SessionConfig`): play-and-record, the
/// voice-chat mode, the loudspeaker rather than the receiver when nothing else is plugged in, and a Bluetooth hands-free
/// headset allowed. The mode is what makes iOS take the audio as a call: it prefers the Bluetooth hands-free route (HFP, a
/// car's call) over A2DP "music". Activating it interrupts other apps' audio (no mix-with-others is declared). `Default`
/// mode was the declaration when the car kept its music (desktop#465; not verified on a device). The voice's own echo
/// cancellation is still the app's: this mode brings none to the audio unit the call opens.
const CALL_AUDIO: SessionConfig = SessionConfig {
    mode: SessionMode::VoiceChat,
    speaker: true,
    bluetooth: true,
};

/// Declares the call's use of the audio session, at the app's start. Activates nothing.
pub fn configure() -> Result<(), String> {
    Session::configure(&CALL_AUDIO)
}

/// How long the first voice call's audio waits for the system to make its call's audio ready (CallKit and Telecom answer in
/// well under a second; Android's call service reports ready by itself before this, Telecom or not). Past it the voice
/// call is refused (`audio-session-activate-failed`): the session is CallKit's to activate while its call lasts, never the
/// app's.
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
    /// The system's call, where the system makes its audio ready ([`call_activates_audio`]).
    system: SystemAudio,
}

/// The audio of the system's call, where the system makes it ready (CallKit activates the session; Telecom makes the call
/// active and settles its route).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum SystemAudio {
    /// No such call: the app activates the session itself.
    #[default]
    None,
    /// A call is starting: the system makes its audio ready when it is.
    Pending,
    /// The system activated the session and has deactivated it since (the call is on hold, another call has the audio).
    Held,
    /// The system has made it ready.
    Active,
}

impl SystemAudio {
    /// The room's call becomes a system's call whose audio the system makes ready (`on`), or stops being one.
    fn call(self, on: bool) -> SystemAudio {
        match (on, self) {
            (false, _) => SystemAudio::None,
            (true, SystemAudio::None) => SystemAudio::Pending,
            (true, now) => now,
        }
    }

    /// The system made its call's audio ready (`active`) or took it away; nothing while there is no such call.
    fn audio(self, active: bool) -> SystemAudio {
        match (self, active) {
            (SystemAudio::None, _) => SystemAudio::None,
            (_, true) => SystemAudio::Active,
            (_, false) => SystemAudio::Held,
        }
    }

    /// What the first voice call's audio does now, `timed_out` once its wait is over.
    fn gate(self, timed_out: bool) -> Gate {
        match self {
            SystemAudio::None => Gate::Activate,
            SystemAudio::Active => Gate::Open,
            SystemAudio::Held => {
                Gate::Refuse("the system's call is on hold; no audio until it is back")
            }
            SystemAudio::Pending if timed_out => {
                Gate::Refuse("the system did not make its call's audio ready in time")
            }
            SystemAudio::Pending => Gate::Wait,
        }
    }
}

/// What the first voice call's audio does, as the system's call stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    /// No system's call: the app activates the session, and the audio opens.
    Activate,
    /// The system made its call's audio ready: the audio opens.
    Open,
    /// The system's call is starting: its word is waited for.
    Wait,
    /// Refused, for the reason given: the app never opens audio the system's call owns before the system says so.
    Refuse(&'static str),
}

/// What a voice call's audio refused for want of the system's session says (sidevoice-voice's code for it).
const ACTIVATE_FAILED: &str = "audio-session-activate-failed";

impl PhoneCall {
    /// Whether a voice call's audio is open, and so the session active.
    fn audio_open(&self) -> bool {
        self.audio.lock().unwrap().open
    }

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

    /// The room's call is a call of the system's that makes its own audio ready (`on`), or is not any more.
    fn system_call(&self, on: bool) {
        let mut audio = self.audio.lock().unwrap();
        audio.system = audio.system.call(on);
        self.changed.notify_all();
    }

    /// The system made its call's audio ready (`active`: CallKit activated the session; Telecom made the call active and
    /// its route settled, or the call stays the app's own) or took it away (CallKit deactivated the session).
    fn system_audio(&self, active: bool) {
        let mut audio = self.audio.lock().unwrap();
        audio.system = audio.system.audio(active);
        self.changed.notify_all();
    }
}

impl CallActivity for PhoneCall {
    /// Before the first voice call's audio opens: the session is declared again, and activated by the app, or (during a
    /// call of the system's that makes its own audio ready) the system's word that it is ready is waited for while the call
    /// starts. While the system holds its call, or past the wait, the voice call is refused: the app never opens audio the
    /// system's call owns before the system says so.
    fn starting(&self) -> Result<(), String> {
        let mut audio = self.audio.lock().unwrap();
        // Declared again first, the same as at the app's start: whatever took the declaration away under an earlier call
        // (a reset of the media services) does not keep this one from recording.
        if let Err(error) = configure() {
            log::warn!("call: the audio session was not declared again: {error}");
        }
        let deadline = Instant::now() + SYSTEM_AUDIO_LIMIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match audio.system.gate(left.is_zero()) {
                Gate::Activate => {
                    Session::activate()?;
                    log::info!("call: the audio opens (no system's call)");
                    break;
                }
                Gate::Open => {
                    log::info!("call: the audio opens: the system's call made it ready");
                    break;
                }
                Gate::Refuse(why) => {
                    log::warn!("call: no audio: {why}");
                    return Err(ACTIVATE_FAILED.into());
                }
                Gate::Wait => {
                    log::info!("call: the audio waits for the system's call to make it ready");
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
            // The reset took the session's declaration with it: without it the call cannot record, now or in any later
            // call, until the app starts again. It is declared again, and activated again if the app holds it; the voice
            // reopens its streams itself. Nothing of the call pauses or resumes.
            Interruption::Reset => {
                match configure() {
                    Ok(()) => log::info!("audio session: declared again after the reset"),
                    Err(error) => log::error!("audio session: not declared again: {error}"),
                }
                phone.reactivate();
                return;
            }
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
    /// Whether the call held is a system's call (CallKit, Telecom: the developer option, read when it started), else the
    /// app's own.
    system: bool,
    /// The call as the page last reported it, with what the app did natively since (a surface's mute, a hang-up).
    state: CallState,
    /// Whether the page's mute cannot be used now (a call with no conversation): a surface's mute does nothing then.
    mic_disabled: bool,
    /// The call the app hung up natively (a surface's hang-up while the page may not run), by its join mark: the page's
    /// reports of that call joined (sent before it ran the hang-up) do not start it again. Cleared by the page's first
    /// report of no call, or of another call.
    hung_up: Option<Option<f64>>,
    /// The join mark of the call the page last reported.
    reported_since: Option<f64>,
    /// The mute the app set natively (a surface's, a headset's), until the page reports it ([`reported_mic`]).
    muting: Option<Muting>,
    /// What the call's surfaces show, from the page's last report (`crate::surface`).
    surface: CallSurface,
    /// What the system's call was last told it shows (`None`: nothing yet).
    shown: Option<CallDetails>,
    /// When it was last told, and whether a later update is already on its way ([`UPDATE_INTERVAL`]).
    updated_at: Option<Instant>,
    update_queued: bool,
    /// The stills the page forgot whose files stay while a surface may still draw them: the one the service last showed
    /// or the one waiting to be shown names them ([`unreferenced`]).
    forgotten: BTreeSet<String>,
}

/// The ids of the stills `surface` draws.
fn still_ids(surface: &CallSurface) -> impl Iterator<Item = &str> {
    let others = surface
        .others
        .iter()
        .filter_map(|other| other.still.as_ref());
    surface
        .still
        .iter()
        .chain(others)
        .map(|still| still.id.as_str())
}

/// Which of the `forgotten` stills none of `surfaces` draws any more: their files can go.
fn unreferenced(forgotten: &BTreeSet<String>, surfaces: &[&CallSurface]) -> Vec<String> {
    let drawn: BTreeSet<&str> = surfaces.iter().copied().flat_map(still_ids).collect();
    forgotten
        .iter()
        .filter(|id| !drawn.contains(id.as_str()))
        .cloned()
        .collect()
}

/// Removes the files of `gone` with `remove`, and forgets the ones removed: a file that failed to go stays pending, to be
/// tried again at the next collection (it would otherwise hold the stills' allowance until the app reloads).
fn retire(
    forgotten: &mut BTreeSet<String>,
    gone: Vec<String>,
    mut remove: impl FnMut(&str) -> std::io::Result<()>,
) {
    for id in gone {
        match remove(&id) {
            Ok(()) => {
                forgotten.remove(&id);
            }
            Err(error) => log::warn!("call_forget: {error}"),
        }
    }
}

/// A forgetting of the still `id` that is pending is cancelled only by a still that was kept (`saved` is `Ok`): a rejected
/// one leaves it to go.
fn keep_if_saved<E>(
    forgotten: &mut BTreeSet<String>,
    id: &str,
    saved: Result<(), E>,
) -> Result<(), E> {
    if saved.is_ok() {
        forgotten.remove(id);
    }
    saved
}

/// How long the page's still waits for the room's thread to keep it.
const STILL_LIMIT: Duration = Duration::from_secs(5);

/// The least time between two updates of the call's service: Android drops a notification's updates past about five a
/// second, and iOS budgets a Live Activity's. A change inside it is shown at its end, with whatever else changed meanwhile.
const UPDATE_INTERVAL: Duration = Duration::from_millis(250);

/// How long a change waits before it is shown, the service last updated at `last` (`None`: not yet).
fn update_wait(last: Option<Instant>, now: Instant) -> Option<Duration> {
    let wait = UPDATE_INTERVAL.checked_sub(now.saturating_duration_since(last?))?;
    (!wait.is_zero()).then_some(wait)
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
            system: false,
            state: CallState::default(),
            mic_disabled: false,
            hung_up: None,
            reported_since: None,
            muting: None,
            surface: CallSurface::default(),
            shown: None,
            updated_at: None,
            update_queued: false,
            forgotten: BTreeSet::new(),
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
            room.mic_disabled = false;
            room.hung_up = None;
            room.reported_since = None;
            room.muting = None;
            room.surface = CallSurface::default();
            room.hold(false, None);
            room.stop_cue();
            // The old page's stills go with it (nothing draws them once its call has ended): the new page sends its own.
            room.forgotten.clear();
            if let Err(error) = clear_stills(&room.app) {
                log::warn!(target: "stills", "call_still: the old page's stills were not removed: {error}");
            }
            // What the old page's interruptions held is let go first; the new page starts with none.
            room.clear_interruption();
            room.interruptions = Hold::new();
            // The media controls let go too.
            room.media();
        });
    }

    /// The page `page` reports its call: whether it is joined (the green and the red button), since when (`since`, the
    /// page's own mark of that call: a join of the same page later is another call), its conversation and its mute
    /// (`state`), whether the mute can be used (`mic_disabled`), and what its surfaces show (`surface`).
    pub fn report(
        &self,
        page: u64,
        state: CallState,
        since: Option<f64>,
        mic_disabled: bool,
        surface: crate::surface::Reported,
    ) {
        self.then(move |room| {
            if room.page == page {
                let mut state = state;
                state.joined = after_hang_up(&mut room.hung_up, state.joined, since);
                let (joined, reported) = (state.joined, state.mic_enabled);
                room.state = state;
                room.mic_disabled = mic_disabled;
                room.page_mic(reported);
                let mic_enabled = room.state.mic_enabled;
                room.reported_since = since;
                room.surface = crate::surface::surface(&surface, &room.surface);
                room.collect();
                room.hold(joined, since);
                room.microphone(mic_enabled, mic_disabled);
                room.show();
                room.media();
            }
        });
    }

    /// The person changed what the headset's double press does (`headset::HeadsetSettings`): the media controls follow, in
    /// order with the rest (iOS registers the next-track command only while the double press has an action).
    pub fn headset_changed(&self) {
        self.then(|room| room.media());
    }

    /// A headset's button or the lock screen asked to mute or unmute: the other of the app's mute (a headset shows none,
    /// [`Room::toggle_mute`]), in order with the rest.
    pub fn toggle_mute(&self) {
        self.then(|room| room.toggle_mute("headset", None));
    }

    /// The person asked for `action` on one of the call's surfaces outside the service's own events (iOS: a link of the Live
    /// Activity the app was opened with): run on the current page's call.
    pub fn surface_action(&self, action: SurfaceAction) {
        self.then(move |room| room.act(action));
    }

    /// The page `page` hands over the still `id`, a PNG of `px` × `px`: kept on the room's thread, in order with the reports
    /// and the forgetting, and answered once its file is there (so the report that names it comes after it). A still of
    /// a page that has been replaced is nothing.
    pub fn save_still(&self, page: u64, id: String, png: Vec<u8>, px: u32) -> Result<(), String> {
        let (done, kept) = mpsc::channel();
        self.then(move |room| {
            let result = if room.page != page {
                log::info!(
                    target: "stills",
                    "call stills: {id} is of page {page}, the room is on {}: dropped",
                    room.page
                );
                Ok(())
            } else {
                let saved = save_still(&room.app, &id, &png, px).map_err(|error| error.to_string());
                keep_if_saved(&mut room.forgotten, &id, saved)
            };
            let _ = done.send(result);
        });
        kept.recv_timeout(STILL_LIMIT)
            .unwrap_or_else(|_| Err("the room's thread did not answer".into()))
    }

    /// The page `page` shows the stills `ids` no more: their files go once no surface draws them ([`unreferenced`]).
    pub fn forget_stills(&self, page: u64, ids: Vec<String>) {
        self.then(move |room| {
            if room.page == page {
                room.forgotten.extend(ids);
                room.collect();
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
    /// Whether a voice call's audio is open (the cue plays only then, `cue` in lib.rs).
    pub fn audio_open(&self) -> bool {
        self.phone.audio_open()
    }

    /// Stops the page's cue (`host.cue`): the call it played in is over, and the page may not run to stop it (a hang-up of the
    /// system's or the service's, a page that reloaded).
    fn stop_cue(&self) {
        let _ = self
            .app
            .run_on_main_thread(|| crate::platform::cue::stop(crate::platform::cue::Cue::Thinking));
    }

    /// Tells the headset's settings who owns the call now (the system, or the app's own), so that the rows and the presses follow
    /// the call and not the developer option, which applies to the next call.
    fn note_owner(&self) {
        if let Some(settings) = self
            .app
            .try_state::<crate::bridge::headset::HeadsetSettings>()
        {
            settings.call_owner(self.holding.map(|_| self.system));
        }
    }

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
                surface: self.surface.clone(),
            };
            // A voice call's audio that opens meanwhile waits for the system to make the call's audio ready (CallKit's
            // session; Telecom's active call and its route).
            // The room's call is the app's own, and waits for no system, unless the developer option turns the system's call on
            // (D-NO-CALLKIT-DEFAULT).
            let system = crate::developer_options::system_call(&self.app);
            // A system's call the system refuses goes on as the app's own, with what the app's own call has (its
            // service, its Live Activity); a refusal of that too leaves the call in front only.
            let kinds: &[bool] = if system { &[true, false] } else { &[false] };
            for &kind in kinds {
                self.phone.system_call(call_activates_audio(kind));
                let app = app.clone();
                match start_call_service(&self.app, &details, kind, move |event| {
                    on_service_event(&app, start, page, since, event)
                }) {
                    Ok(()) => {
                        self.holding = Some(start);
                        self.system = kind;
                        self.note_owner();
                        self.since = since;
                        // The system is told the page's mute and title at once (`show`), whatever it started with.
                        self.shown = None;
                        log::info!("call: started{}", if kind { " (the system's)" } else { "" });
                        break;
                    }
                    // The call goes on as the app's own (in front on Android: it does not outlive the screen; on iOS with
                    // the app's own audio session and media controls). Recorded by its code (`not-visible`,
                    // `microphone-permission`, `call-refused`, …).
                    Err(error) => {
                        self.phone.system_call(false);
                        log::error!(
                            "call: the {} call did not start: {error}",
                            if kind { "system's" } else { "app's own" }
                        );
                    }
                }
            }
        } else {
            self.holding = None;
            self.system = false;
            self.note_owner();
            self.clear_interruption();
            self.stop_cue();
            self.since = None;
            self.shown = None;
            self.phone.system_call(false);
            if let Err(error) = stop_call_service(&self.app) {
                log::warn!("call: the call's service did not stop: {error}");
            }
            log::info!("call: ended");
        }
    }

    /// Tells the call's service what it shows now, when that changed: the page's conversation, its mute and the call's
    /// surface. At most once every [`UPDATE_INTERVAL`]: a change sooner is shown at its end.
    fn show(&mut self) {
        self.tell(false);
    }

    /// Tells the call's service what it shows now, at once and changed or not, and has its surface drawn again even where
    /// it shows that already (iOS: the Live Activity's toggle, which flips its own face when tapped): after a tap.
    fn refresh(&mut self) {
        self.updated_at = None;
        self.tell(true);
    }

    fn tell(&mut self, again: bool) {
        if self.holding.is_none() {
            return;
        }
        let details = CallDetails {
            title: self.state.title.trim().to_owned(),
            muted: !self.state.mic_enabled,
            surface: self.surface.clone(),
        };
        if !again && self.shown.as_ref() == Some(&details) {
            return;
        }
        if let Some(wait) = update_wait(self.updated_at, Instant::now()) {
            self.show_later(wait);
            return;
        }
        let told = if again {
            refresh_call_service(&self.app, &details)
        } else {
            update_call_service(&self.app, &details)
        };
        match told {
            Ok(()) => {
                self.shown = Some(details);
                self.updated_at = Some(Instant::now());
                self.collect();
            }
            Err(error) => log::warn!("call: the system's call was not updated: {error}"),
        }
    }

    /// Removes the files of the forgotten stills that neither the surface shown nor the one waiting to be shown draws.
    fn collect(&mut self) {
        let shown = self.shown.as_ref().map(|shown| &shown.surface);
        let surfaces: Vec<&CallSurface> = shown.into_iter().chain([&self.surface]).collect();
        let gone = unreferenced(&self.forgotten, &surfaces);
        let app = self.app.clone();
        retire(&mut self.forgotten, gone, |id| {
            forget_stills(&app, &[id.to_owned()])
        });
    }

    /// Shows the call again in `wait`, once, however many changes come meanwhile.
    fn show_later(&mut self, wait: Duration) {
        if self.update_queued {
            return;
        }
        self.update_queued = true;
        let app = self.app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(wait);
            if let Some(room) = app.try_state::<RoomCall>() {
                room.then(|room| {
                    room.update_queued = false;
                    room.show();
                });
            }
        });
    }

    /// Runs what the person asked for on a surface. Each action has one native owner, the same the page's own button ends
    /// in, and the app calls it itself: with the screen locked the WebView may not run script, so nothing waits for the
    /// page ([`native_action`]). The page is still told, as its own command for its call (its number, and the call's join
    /// mark when its service holds it), and runs it when it runs again: it then shows what was done (its mute, the call
    /// ended), records it as the person's, and does what only it can (tell the room). Asked first, so that the page has it
    /// before anything the native side makes the call tell it.
    fn act(&mut self, action: SurfaceAction) {
        let native = native_action(action);
        log::info!(target: "intent", "call: {action:?} from the call's surface; natively: {native:?}");
        // The page's mute command is sent by `set_mute`, which every native mute goes through.
        if let Some(mut request) = surface_command(action) {
            request["page"] = self.page.into();
            if let Some(since) = self.since {
                request["since"] = since.into();
            }
            run_on_page(&self.app, request);
        }
        match native {
            NativeAction::Mute(muted) => {
                self.set_mute(muted);
            }
            NativeAction::ToggleMute => {
                let showed = self.shown.as_ref().map(|shown| shown.muted);
                self.toggle_mute("surface", showed);
            }
            NativeAction::CancelInput => {
                self.with_host("cancel-send", |host| async move {
                    host.cancel_microphone_input().await
                });
            }
            NativeAction::Skip => {
                self.with_host("skip", |host| async move {
                    host.skip_playing().await;
                    true
                });
            }
            NativeAction::HangUp => self.hang_up(),
            NativeAction::None => {}
        }
    }

    /// A mute toggle was tapped (`by`: the Live Activity's, a headset's): the mute is set to the other of the one it showed
    /// (`showed`: the mute the app last told it, its face when it was tapped), or of the app's own when it shows none
    /// ([`toggled`]). Never from the value the toggle sends, which a locked phone has sent wrong. The call is drawn again,
    /// changed or not: the toggle flipped its own face, and a tap that changed nothing must not leave it flipped.
    fn toggle_mute(&mut self, by: &str, showed: Option<bool>) {
        let has = !self.state.mic_enabled;
        let muted = toggled(showed, has);
        log::info!(
            target: "intent",
            "call: toggle-mute from the {by}: it showed muted={}, the app has muted={has}, so muted={muted}{}",
            showed.map_or_else(|| "none".to_owned(), |showed| showed.to_string()),
            if muted == has { ", as it is already" } else { "" }
        );
        // A mute set is shown at once (`set_mute`); anything else is drawn again here.
        if muted == has || !self.set_mute(muted) {
            self.refresh();
        }
    }

    /// Runs `work` on the voice host, off the room's thread, and logs whether it found a call (`what` names it).
    fn with_host<F>(&self, what: &'static str, work: impl FnOnce(AppHost) -> F + Send + 'static)
    where
        F: std::future::Future<Output = bool> + Send + 'static,
    {
        let app = self.app.clone();
        tauri::async_runtime::spawn(async move {
            let Some(host) = app.try_state::<AppHost>().map(|host| host.inner().clone()) else {
                log::warn!(target: "intent", "call: {what}: no voice host");
                return;
            };
            let found = work(host).await;
            log::info!(
                target: "intent",
                "call: {what} done natively{}",
                if found { "" } else { " (no call to act on)" }
            );
        });
    }

    /// Mutes or unmutes the call's microphone: the voice host's own mute of the room's call (the lane the microphone is
    /// routed to; the same mute the page's button makes through `voice_mute`), at once and without the page. The call's
    /// surfaces show it at once (the Live Activity's toggle and the system's call agree with it), and the page is told,
    /// to show it and keep it as the person's when it runs. Nothing while the page's mute cannot be used. Whether it was
    /// set.
    fn set_mute(&mut self, muted: bool) -> bool {
        if self.mic_disabled {
            log::info!(target: "intent", "call: mute {muted}: the call's mute cannot be used now");
            return false;
        }
        self.state.mic_enabled = !muted;
        self.muting = Some(Muting { muted, held: None });
        let mut request =
            serde_json::json!({ "command": "mute", "page": self.page, "muted": muted });
        if let Some(since) = self.since {
            request["since"] = since.into();
        }
        run_on_page(&self.app, request);
        self.with_host(
            if muted { "mute" } else { "unmute" },
            move |host| async move { host.mute_microphone(muted).await },
        );
        // Shown at once, the pill, the tile's badge and the island's glyph in one update: the person is looking at it.
        self.updated_at = None;
        self.show();
        self.media();
        true
    }

    /// Takes the page's report of the microphone (`reported`), or not while a native mute waits for the page
    /// ([`reported_mic`]), and says which.
    fn page_mic(&mut self, reported: bool) {
        let (mic, report) = reported_mic(
            &mut self.muting,
            reported,
            self.mic_disabled,
            self.state.joined,
            Instant::now(),
        );
        let word = |on: bool| if on { "on" } else { "off" };
        match report {
            MicReport::Taken => {}
            MicReport::Held { first } => {
                log::info!(
                    target: "intent",
                    "call: the page reports the microphone {}: kept {}, a native mute waits for the page to run it",
                    word(reported),
                    word(mic)
                );
                if first {
                    self.mute_wait_later();
                }
            }
            MicReport::TimedOut => log::warn!(
                target: "intent",
                "call: the page still reports the microphone {} {MUTE_WAIT:?} after it reported again: taken, it did not run the native mute",
                word(reported)
            ),
        }
        self.state.mic_enabled = mic;
    }

    /// Looks again in [`MUTE_WAIT`]: a page that reports nothing more has its last report taken then, if the mute still
    /// waits for it.
    fn mute_wait_later(&self) {
        let app = self.app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(MUTE_WAIT);
            if let Some(room) = app.try_state::<RoomCall>() {
                room.then(Room::mute_wait_over);
            }
        });
    }

    fn mute_wait_over(&mut self) {
        let Some(Muting {
            held: Some((since, reported)),
            ..
        }) = self.muting
        else {
            return;
        };
        if since.elapsed() < MUTE_WAIT {
            return; // another mute's wait, which looks again itself
        }
        self.page_mic(reported);
        self.microphone(self.state.mic_enabled, self.mic_disabled);
        self.show();
        self.media();
    }

    /// Hangs the call up natively (a surface's hang-up, the system's call ended from outside): its audio stops (what is
    /// said is dropped, every call of the page stops listening and speaking, the microphone is routed nowhere, so the
    /// audio session is given back), its service and Live Activity end, and the page's reports of that call joined are
    /// not taken for it any more ([`after_hang_up`]). The page finishes the hang-up when it runs (the room told, its
    /// lanes disposed of).
    fn hang_up(&mut self) {
        if self.holding.is_none() && !self.state.joined {
            log::info!(target: "intent", "call: hang-up: no call");
            return;
        }
        self.hung_up = Some(self.reported_since);
        self.state.joined = false;
        self.hold(false, None);
        self.media();
        self.stop_audio();
    }

    /// The voice host stops what the page's calls say and hear, without the page.
    fn stop_audio(&self) {
        self.with_host("hang-up", |host| async move {
            host.skip_playing().await;
            host.stop_calls().await > 0
        });
    }

    /// The media controls follow the page's call (the media controls reach the plugin's Kotlin, which runs on Android's
    /// UI thread: never from there). Where the system's call takes their place, they show none while it lasts.
    fn media(&self) {
        // What the person chose for the headset: a double press with an action registers the next-track command, and any choice but
        // the defaults needs the remote commands, so Now Playing stays beside the call's own surface (iOS: the Live Activity). With
        // the defaults it gives way, and mute comes from the system's mute gesture and the surface's button.
        let settings = self
            .app
            .try_state::<crate::bridge::headset::HeadsetSettings>();
        let wanted = settings
            .as_ref()
            .is_some_and(|settings| settings.double_press_wanted(self.system));
        let choice = settings
            .as_ref()
            .map(|settings| settings.choice())
            .unwrap_or_default();
        let state = if self.holding.is_some() && self.system && call_replaces_media_controls(true) {
            CallState {
                ready: self.state.ready,
                ..CallState::default()
            }
        } else if self.holding.is_some() && !self.system && call_replaces_media_controls(false) {
            // The call's Live Activity shows it: no second card (unless the person chose another action, which only the remote
            // commands of Now Playing deliver), but the AirPods gesture still follows the call.
            CallState {
                now_playing_off: sidevoice_shared_headset::now_playing_gives_way(true, choice),
                double_press_wanted: wanted,
                ..self.state.clone()
            }
        } else {
            CallState {
                double_press_wanted: wanted,
                ..self.state.clone()
            }
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
        // Muted or unmuted from the system's side: the microphone's native mute, and the page's, set to that (nothing if it
        // is so already). The system shows it already: it is not told back.
        CallEvent::Muted { muted } => {
            room.then(move |room| {
                if mine(room) {
                    if let Some(shown) = room.shown.as_mut() {
                        shown.muted = muted;
                    }
                    room.set_mute(muted);
                }
            });
            return;
        }
        // The system made its call's audio ready, or took it away: a voice call's audio may be waiting for it.
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
        // Asked for on one of the call's surfaces (the notification, the Live Activity): run on the page's call.
        CallEvent::Action { action } => {
            room.then(move |room| {
                if mine(room) {
                    room.act(action);
                } else {
                    log::info!(
                        target: "intent",
                        "call: {action:?} dropped: it is of start {start} on page {page}, the room holds {:?} on page {}",
                        room.holding,
                        room.page
                    );
                    room.refresh();
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
            // The page first, so that it has the hang-up before what the stopped calls tell it.
            run_on_page(
                &room.app,
                serde_json::json!({ "command": "hang-up", "page": page, "since": since }),
            );
            // The service is gone: the next join starts another.
            room.holding = None;
            room.system = false;
            room.note_owner();
            room.stop_cue();
            room.shown = None;
            room.phone.system_call(false);
            room.clear_interruption();
            room.since = None;
            // Hung up natively too, as a surface's hang-up is: the page may not run (the screen locked).
            room.hung_up = Some(since);
            room.state.joined = false;
            room.media();
            room.stop_audio();
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
        | CallEvent::Audio { .. }
        | CallEvent::Action { .. } => None,
    }
}

/// The page's command for what the person asked on a surface (mobile/tauri/js/mobile-bridge.js, sidevoice-shared-tauri's
/// members). None for the mute: [`Room::set_mute`] tells the page the mute it set, as a value (a toggle would have the
/// page flip again what the app has set already).
fn surface_command(action: SurfaceAction) -> Option<serde_json::Value> {
    let command = |name: &str| Some(serde_json::json!({ "command": name }));
    match action {
        SurfaceAction::Mute | SurfaceAction::Unmute | SurfaceAction::ToggleMute => None,
        SurfaceAction::Skip => command("skip-reply"),
        SurfaceAction::CancelSend => command("cancel-input"),
        SurfaceAction::HangUp => command("hang-up"),
        SurfaceAction::OpenConversations => command("open-conversations"),
    }
}

/// What the app does itself for an action asked on a surface, through the owner the page's own button ends in too: the
/// voice host's mute of the room's call (`voice_mute`), its cancel of the unreported turn (`voice_cancel_input`), its
/// cancel of what is said (`voice_cancel_say`), and the call's end (the room's service, and the calls' stop).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeAction {
    Mute(bool),
    /// The other of the mute the surface showed ([`Room::toggle_mute`]).
    ToggleMute,
    CancelInput,
    Skip,
    HangUp,
    /// Nothing native: only the page can (open its list of conversations, with the app in front).
    None,
}

fn native_action(action: SurfaceAction) -> NativeAction {
    match action {
        SurfaceAction::Mute => NativeAction::Mute(true),
        SurfaceAction::Unmute => NativeAction::Mute(false),
        SurfaceAction::ToggleMute => NativeAction::ToggleMute,
        SurfaceAction::Skip => NativeAction::Skip,
        SurfaceAction::CancelSend => NativeAction::CancelInput,
        SurfaceAction::HangUp => NativeAction::HangUp,
        SurfaceAction::OpenConversations => NativeAction::None,
    }
}

/// The mute a tap on a mute toggle asks for: the other of the one it showed, or of the app's (`muted`) when it shows none.
fn toggled(showed: Option<bool>, muted: bool) -> bool {
    !showed.unwrap_or(muted)
}

/// How long the page may go on reporting the other mute, once it reports again after a mute the app set natively, before
/// its report is taken: a page that runs has run the mute by then, so one that still disagrees did not take it (its
/// command refused, its toggle failed). Counted from its first such report, not from the mute: a locked phone's page may
/// not run, nor report, for minutes.
const MUTE_WAIT: Duration = Duration::from_secs(3);

/// A mute the app set natively, waiting for the page to report it ([`reported_mic`]).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Muting {
    muted: bool,
    /// Since when the page has reported the other mute, and the microphone it last reported (`None`: not yet).
    held: Option<(Instant, bool)>,
}

/// What became of a page's report of the microphone ([`reported_mic`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MicReport {
    /// Taken: no native mute waits, or the report ends the wait.
    Taken,
    /// Not taken: the page has not run the mute yet. `first`: its first such report since the mute.
    Held { first: bool },
    /// Taken: the page has reported the other mute for [`MUTE_WAIT`]; the wait ends.
    TimedOut,
}

/// The microphone a page's report says is on (`reported`, at `now`), after the app set the mute natively (`muting`): a
/// report that disagrees is one the page sent before it ran that mute (a surface's, while the page did not run), and is
/// not taken, for up to [`MUTE_WAIT`]; one that agrees, or one of a mute that cannot be used or of no call, is the page's
/// again and ends the wait.
fn reported_mic(
    muting: &mut Option<Muting>,
    reported: bool,
    disabled: bool,
    joined: bool,
    now: Instant,
) -> (bool, MicReport) {
    match muting {
        Some(wait) if joined && !disabled && reported == wait.muted => {
            let muted = wait.muted;
            let first = wait.held.is_none();
            let since = wait.held.map_or(now, |(since, _)| since);
            wait.held = Some((since, reported));
            if now.saturating_duration_since(since) >= MUTE_WAIT {
                *muting = None;
                (reported, MicReport::TimedOut)
            } else {
                (!muted, MicReport::Held { first })
            }
        }
        _ => {
            *muting = None;
            (reported, MicReport::Taken)
        }
    }
}

/// Whether the page's report of a call `joined` at `since` holds the call, the app having hung up natively the call joined
/// at `hung_up`: not while it reports that call joined (it has not run the hang-up yet); its report of no call, or of
/// another call, is its own again and forgets the hang-up.
fn after_hang_up(hung_up: &mut Option<Option<f64>>, joined: bool, since: Option<f64>) -> bool {
    match *hung_up {
        Some(ended) if joined && ended == since => false,
        _ => {
            *hung_up = None;
            joined
        }
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
            CallEvent::Action {
                action: SurfaceAction::HangUp,
            },
        ] {
            assert_eq!(hang_up_for(&event), None, "{event:?}");
        }
    }

    #[test]
    fn a_surface_s_action_is_the_page_s_own_command() {
        for action in [
            SurfaceAction::Mute,
            SurfaceAction::Unmute,
            SurfaceAction::ToggleMute,
        ] {
            assert_eq!(surface_command(action), None, "{action:?}");
        }
        for (action, command) in [
            (SurfaceAction::Skip, r#"{"command":"skip-reply"}"#),
            (SurfaceAction::CancelSend, r#"{"command":"cancel-input"}"#),
            (SurfaceAction::HangUp, r#"{"command":"hang-up"}"#),
            (
                SurfaceAction::OpenConversations,
                r#"{"command":"open-conversations"}"#,
            ),
        ] {
            assert_eq!(
                surface_command(action),
                Some(serde_json::from_str::<serde_json::Value>(command).unwrap()),
                "{action:?}"
            );
        }
    }

    #[test]
    fn every_call_action_of_a_surface_has_a_native_owner() {
        for (action, native) in [
            (SurfaceAction::Mute, NativeAction::Mute(true)),
            (SurfaceAction::Unmute, NativeAction::Mute(false)),
            (SurfaceAction::ToggleMute, NativeAction::ToggleMute),
            (SurfaceAction::Skip, NativeAction::Skip),
            (SurfaceAction::CancelSend, NativeAction::CancelInput),
            (SurfaceAction::HangUp, NativeAction::HangUp),
            (SurfaceAction::OpenConversations, NativeAction::None),
        ] {
            assert_eq!(native_action(action), native, "{action:?}");
        }
    }

    #[test]
    fn a_tap_on_a_mute_toggle_does_what_it_showed() {
        for has in [false, true] {
            assert!(toggled(Some(false), has), "it showed live: mute");
            assert!(!toggled(Some(true), has), "it showed muted: unmute");
        }
        assert!(toggled(None, false), "a headset: the other of the app's");
        assert!(!toggled(None, true));
    }

    #[test]
    fn a_mute_set_natively_is_not_undone_by_the_page_s_late_reports() {
        let t0 = Instant::now();
        let wait = |muted| Some(Muting { muted, held: None });
        let mut muting = wait(true);
        assert_eq!(
            reported_mic(&mut muting, true, false, true, t0),
            (false, MicReport::Held { first: true }),
            "a report from before the mute"
        );
        assert_eq!(
            reported_mic(&mut muting, true, false, true, t0 + Duration::from_secs(1)),
            (false, MicReport::Held { first: false }),
            "kept until the page follows"
        );
        assert_eq!(
            reported_mic(&mut muting, false, false, true, t0 + Duration::from_secs(2)),
            (false, MicReport::Taken),
            "the page muted"
        );
        assert_eq!(muting, None);
        assert_eq!(
            reported_mic(&mut muting, true, false, true, t0),
            (true, MicReport::Taken),
            "the person unmuted in the app"
        );

        let mut muting = wait(false);
        assert_eq!(
            reported_mic(&mut muting, false, false, true, t0),
            (true, MicReport::Held { first: true }),
            "an unmute too"
        );
        assert_eq!(
            reported_mic(&mut muting, true, false, true, t0),
            (true, MicReport::Taken)
        );
        assert_eq!(muting, None);

        for (disabled, joined) in [(true, true), (false, false)] {
            let mut muting = wait(true);
            assert_eq!(
                reported_mic(&mut muting, true, disabled, joined, t0),
                (true, MicReport::Taken)
            );
            assert_eq!(muting, None, "disabled {disabled}, joined {joined}");
        }
    }

    #[test]
    fn a_page_that_does_not_follow_a_native_mute_is_taken_again_in_a_few_seconds() {
        let t0 = Instant::now();
        let mut muting = Some(Muting {
            muted: true,
            held: None,
        });
        // Counted from the page's first report after the mute, however long the phone was locked before it.
        assert_eq!(
            reported_mic(&mut muting, true, false, true, t0),
            (false, MicReport::Held { first: true })
        );
        let almost = t0 + MUTE_WAIT - Duration::from_millis(1);
        assert!(!reported_mic(&mut muting, true, false, true, almost).0);
        assert_eq!(
            reported_mic(&mut muting, true, false, true, t0 + MUTE_WAIT),
            (true, MicReport::TimedOut)
        );
        assert_eq!(muting, None);
    }

    #[test]
    fn a_call_hung_up_natively_is_not_started_again_by_the_page_s_late_reports() {
        let mut hung_up = Some(Some(1.0));
        assert!(
            !after_hang_up(&mut hung_up, true, Some(1.0)),
            "that call, still joined"
        );
        assert_eq!(hung_up, Some(Some(1.0)), "kept until the page follows");
        assert!(
            !after_hang_up(&mut hung_up, false, None),
            "the page hung up"
        );
        assert_eq!(hung_up, None);
        assert!(
            after_hang_up(&mut hung_up, true, Some(2.0)),
            "the next call holds"
        );

        let mut hung_up = Some(Some(1.0));
        assert!(
            after_hang_up(&mut hung_up, true, Some(2.0)),
            "another call holds"
        );
        assert_eq!(hung_up, None);

        // A call the service never held (no join mark).
        let mut hung_up = Some(None);
        assert!(!after_hang_up(&mut hung_up, true, None));
        let mut none = None;
        assert!(after_hang_up(&mut none, true, Some(3.0)));
    }

    #[test]
    fn a_forgotten_still_s_file_stays_while_a_surface_draws_it() {
        use sidevoice_mobile_plugin::{Other, Still};
        let still = |id: &str| {
            Some(Still {
                id: id.into(),
                k: 1.0,
                file: String::new(),
            })
        };
        let shown = CallSurface {
            still: still("t1#240"),
            others: vec![Other {
                title: "B".into(),
                still: still("t2#108"),
            }],
            ..CallSurface::default()
        };
        let next = CallSurface {
            still: still("t1#240"),
            ..CallSurface::default()
        };
        let forgotten: BTreeSet<String> = ["t2#108", "t3#108"].map(String::from).into();
        // The service still shows t2's face, waiting for the next update: only t3 goes.
        assert_eq!(unreferenced(&forgotten, &[&shown, &next]), ["t3#108"]);
        // Once the update without it is shown, t2 goes too.
        assert_eq!(
            unreferenced(&forgotten, &[&next, &next]),
            ["t2#108", "t3#108"]
        );
        assert!(unreferenced(&BTreeSet::new(), &[&shown]).is_empty());
    }

    #[test]
    fn the_service_is_updated_at_most_once_an_interval() {
        let now = Instant::now();
        assert_eq!(
            update_wait(None, now),
            None,
            "the first update goes at once"
        );
        assert_eq!(update_wait(Some(now), now), Some(UPDATE_INTERVAL));
        let later = now + UPDATE_INTERVAL / 5;
        assert_eq!(update_wait(Some(now), later), Some(UPDATE_INTERVAL * 4 / 5));
        assert_eq!(update_wait(Some(now), now + UPDATE_INTERVAL), None);
        assert_eq!(update_wait(Some(now), now + UPDATE_INTERVAL * 3), None);
    }

    #[test]
    fn without_a_system_s_call_the_app_activates_the_session_itself() {
        assert_eq!(SystemAudio::None.gate(false), Gate::Activate);
        // A report for a call that is not the system's any more changes nothing.
        assert_eq!(SystemAudio::None.audio(true), SystemAudio::None);
        assert_eq!(SystemAudio::None.call(false), SystemAudio::None);
    }

    #[test]
    fn an_app_owned_call_activates_voice_chat_and_a_system_owned_one_waits() {
        // What the room tells the phone at a call's start is `system_call(call_activates_audio(kind))`: the app's own call
        // is no system's call, so the app activates the session (voice chat, `CALL_AUDIO`); CallKit's is activated by the
        // system, and the app waits for it.
        assert!(!call_activates_audio(false));
        let own = SystemAudio::None.call(call_activates_audio(false));
        assert_eq!(own.gate(false), Gate::Activate);
        assert_eq!(CALL_AUDIO.mode, SessionMode::VoiceChat);
        assert_eq!(SystemAudio::None.call(true).gate(false), Gate::Wait);
        // A stale system's call (it ended, or the system refused it and it went on as the app's own) never keeps the app
        // from activating its session.
        assert_eq!(SystemAudio::Active.call(false).gate(false), Gate::Activate);
        assert_eq!(SystemAudio::Held.call(false).gate(false), Gate::Activate);
    }

    #[test]
    fn the_audio_of_a_system_s_call_waits_until_the_system_makes_it_ready() {
        let starting = SystemAudio::None.call(true);
        assert_eq!(starting, SystemAudio::Pending);
        assert_eq!(starting.gate(false), Gate::Wait);
        // CallKit's session activated; Telecom's call active with its route settled, or Telecom refusing it.
        let ready = starting.audio(true);
        assert_eq!(ready.gate(false), Gate::Open);
        assert_eq!(ready.gate(true), Gate::Open);
        // The join reported again does not take the readiness back.
        assert_eq!(ready.call(true), SystemAudio::Active);
    }

    #[test]
    fn past_the_wait_or_on_hold_the_audio_is_refused() {
        assert!(matches!(SystemAudio::Pending.gate(true), Gate::Refuse(_)));
        let held = SystemAudio::Active.audio(false);
        assert_eq!(held, SystemAudio::Held);
        assert!(matches!(held.gate(false), Gate::Refuse(_)));
        assert_eq!(held.audio(true).gate(false), Gate::Open);
    }

    #[test]
    fn the_call_s_end_gives_the_session_back_to_the_app() {
        for now in [SystemAudio::Pending, SystemAudio::Active, SystemAudio::Held] {
            assert_eq!(now.call(false).gate(false), Gate::Activate, "{now:?}");
        }
    }

    #[test]
    fn a_waiting_start_opens_when_the_system_says_ready() {
        let phone = Arc::new(PhoneCall::default());
        phone.system_call(true);
        let waiting = {
            let phone = phone.clone();
            std::thread::spawn(move || phone.starting())
        };
        std::thread::sleep(Duration::from_millis(50));
        assert!(!waiting.is_finished());
        phone.system_audio(true);
        assert_eq!(waiting.join().unwrap(), Ok(()));
        assert!(phone.audio.lock().unwrap().open);
    }

    #[test]
    fn a_start_while_the_system_s_call_is_held_is_refused() {
        let phone = PhoneCall::default();
        phone.system_call(true);
        phone.system_audio(true);
        phone.system_audio(false);
        assert_eq!(phone.starting(), Err(ACTIVATE_FAILED.to_owned()));
        assert!(!phone.audio.lock().unwrap().open);
    }

    #[test]
    fn the_session_is_play_and_record_in_voice_chat_on_the_loudspeaker_with_bluetooth() {
        assert_eq!(
            CALL_AUDIO,
            SessionConfig {
                mode: SessionMode::VoiceChat,
                speaker: true,
                bluetooth: true
            }
        );
    }

    #[test]
    fn a_still_whose_file_did_not_go_stays_pending() {
        let mut forgotten: BTreeSet<String> = ["a", "b", "c"].map(String::from).into();
        let gone = ["a", "b"].map(String::from).to_vec();
        retire(&mut forgotten, gone.clone(), |id| match id {
            "a" => Err(std::io::Error::other("busy")),
            _ => Ok(()),
        });
        assert_eq!(forgotten, ["a", "c"].map(String::from).into());
        retire(&mut forgotten, gone, |_| Ok(()));
        assert_eq!(forgotten, ["c"].map(String::from).into());
    }

    #[test]
    fn only_a_kept_still_cancels_its_pending_forgetting() {
        let mut forgotten: BTreeSet<String> = ["a"].map(String::from).into();
        assert_eq!(
            keep_if_saved(&mut forgotten, "a", Err("image")),
            Err("image")
        );
        assert!(forgotten.contains("a"));
        assert_eq!(keep_if_saved::<&str>(&mut forgotten, "a", Ok(())), Ok(()));
        assert!(forgotten.is_empty());
    }
}
