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
    request_call_focus, save_still, start_call_service, stop_call_service, update_call_service,
    CallDetails, CallEvent, CallSurface, StopReason, SurfaceAction,
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
    /// The call as the page last reported it.
    state: CallState,
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
fn kept<E>(forgotten: &mut BTreeSet<String>, id: &str, saved: Result<(), E>) -> Result<(), E> {
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
            room.surface = CallSurface::default();
            room.hold(false, None);
            // The old page's stills go with it (nothing draws them once its call has ended): the new page sends its own.
            room.forgotten.clear();
            if let Err(error) = clear_stills(&room.app) {
                log::warn!("call_still: the old page's stills were not removed: {error}");
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
                let (joined, mic_enabled) = (state.joined, state.mic_enabled);
                room.state = state;
                room.surface = crate::surface::surface(&surface, &room.surface);
                room.collect();
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
                Ok(())
            } else {
                let saved = save_still(&room.app, &id, &png, px).map_err(|error| error.to_string());
                kept(&mut room.forgotten, &id, saved)
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
            let system_audio = call_activates_audio(system);
            self.phone.system_call(system_audio);
            match start_call_service(&self.app, &details, system, move |event| {
                on_service_event(&app, start, page, since, event)
            }) {
                Ok(()) => {
                    self.holding = Some(start);
                    self.system = system;
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
            self.system = false;
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

    /// Tells the call's service what it shows now, when that changed: the page's conversation, its mute and the call's
    /// surface. At most once every [`UPDATE_INTERVAL`]: a change sooner is shown at its end.
    fn show(&mut self) {
        if self.holding.is_none() {
            return;
        }
        let details = CallDetails {
            title: self.state.title.trim().to_owned(),
            muted: !self.state.mic_enabled,
            surface: self.surface.clone(),
        };
        if self.shown.as_ref() == Some(&details) {
            return;
        }
        if let Some(wait) = update_wait(self.updated_at, Instant::now()) {
            self.show_later(wait);
            return;
        }
        match update_call_service(&self.app, &details) {
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

    /// Runs what the person asked for on a surface on the page's call (its number, and the call's join mark when its
    /// service holds it), as the page's own command.
    fn act(&self, action: SurfaceAction) {
        log::info!("call: {action:?} from the call's surface");
        let mut request = surface_command(action);
        request["page"] = self.page.into();
        if let Some(since) = self.since {
            request["since"] = since.into();
        }
        run_on_page(&self.app, request);
    }

    /// The media controls follow the page's call (the media controls reach the plugin's Kotlin, which runs on Android's
    /// UI thread: never from there). Where the system's call takes their place, they show none while it lasts.
    fn media(&self) {
        let state = if self.holding.is_some() && call_replaces_media_controls(self.system) {
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
            room.system = false;
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
        | CallEvent::Audio { .. }
        | CallEvent::Action { .. } => None,
    }
}

/// The page's command for what the person asked on a surface (mobile/tauri/js/mobile-bridge.js, sidevoice-shared-tauri's
/// members): the mute as the person's own, set to a value rather than toggled, so that a tap on a surface that showed a
/// stale mute does what it said.
fn surface_command(action: SurfaceAction) -> serde_json::Value {
    let command = |name: &str| serde_json::json!({ "command": name });
    match action {
        SurfaceAction::Mute => serde_json::json!({ "command": "mute", "muted": true }),
        SurfaceAction::Unmute => serde_json::json!({ "command": "mute", "muted": false }),
        SurfaceAction::Skip => command("skip-reply"),
        SurfaceAction::CancelSend => command("cancel-input"),
        SurfaceAction::HangUp => command("hang-up"),
        SurfaceAction::OpenConversations => command("open-conversations"),
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
        for (action, command) in [
            (SurfaceAction::Mute, r#"{"command":"mute","muted":true}"#),
            (SurfaceAction::Unmute, r#"{"command":"mute","muted":false}"#),
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
                serde_json::from_str::<serde_json::Value>(command).unwrap(),
                "{action:?}"
            );
        }
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
        assert_eq!(kept(&mut forgotten, "a", Err("image")), Err("image"));
        assert!(forgotten.contains("a"));
        assert_eq!(kept::<&str>(&mut forgotten, "a", Ok(())), Ok(()));
        assert!(forgotten.is_empty());
    }
}
