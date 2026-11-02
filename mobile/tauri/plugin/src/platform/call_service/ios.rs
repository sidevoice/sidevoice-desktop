//! iOS: the call is a CallKit call (Apple's "Making and receiving VoIP calls"). One `CXProvider` for the app, made at its
//! start: audio only, one call at a time, not listed in the Phone app's Recents, the app's mark as its icon. The green button
//! asks for a `CXStartCallAction` through a `CXCallController`, with the conversation as the handle; the provider reports it
//! connecting, and connected once the system has activated its audio. The red button asks for a `CXEndCallAction`; one the
//! app did not ask for (the system's in-call screen, the lock screen, a headset, a car kit) is the person hanging up.
//!
//! The audio session is the system's to activate for a CallKit call: the app declares it at its start (as without CallKit)
//! and starts its audio once `provider(_:didActivate:)` says so (`CallEvent::Audio`, which the app's call waits for). Mute
//! (`CXSetMutedCallAction`) and hold (`CXSetHeldCallAction`, another call taking this one's place) from outside the app are
//! reported to it; the page's own mute is told to the system as a `CXSetMutedCallAction` the app asks for.
//!
//! Every CallKit object lives on the main thread: the provider's delegate is called there (no queue of its own), the
//! controller answers there (made with the main queue: its own default is a private one), and the app's requests are carried there (`run_on_main_thread`). The calls of this body come
//! from the app's call thread, never from the main thread, which `start` waits on.
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AllocAnyThread, MainThreadMarker, MainThreadOnly};
use objc2_avf_audio::AVAudioSession;
use objc2_call_kit::{
    CXAction, CXCallController, CXCallUpdate, CXEndCallAction, CXHandle, CXHandleType, CXProvider,
    CXProviderConfiguration, CXProviderDelegate, CXSetHeldCallAction, CXSetMutedCallAction,
    CXStartCallAction,
};
use objc2_foundation::{
    NSData, NSError, NSNumber, NSObject, NSObjectProtocol, NSSet, NSString, NSUUID,
};
use serde::de::DeserializeOwned;
use tauri::{plugin::PluginApi, AppHandle, Runtime};

use super::Os;
use crate::{CallDetails, CallEvent, CallEvents, CallServiceError, StopReason};

/// The icon the system shows for the app's calls (`iconTemplateImageData`): the Sidevoice mark in one ink, its alpha the
/// shape (scripts/make-icons.mjs draws it).
const ICON: &[u8] = include_bytes!("../../../assets/call-icon.png");

/// What the call shows as its other party when it is on no conversation yet: the app's name (a name, not a text to
/// translate).
const NO_CONVERSATION: &str = "Sidevoice";

/// How long the green button waits for CallKit to take the call: it answers at once, unless the system is busy.
const START_LIMIT: Duration = Duration::from_secs(5);

pub struct Native;

impl Os for Native {
    const ACTIVATES_AUDIO: bool = true;
    const REPLACES_MEDIA_CONTROLS: bool = true;

    fn register<R: Runtime, C: DeserializeOwned>(
        api: PluginApi<R, C>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        api.app().run_on_main_thread(|| {
            if let Some(main) = MainThreadMarker::new() {
                CALLKIT.with_borrow_mut(|kit| *kit = Some(CallKit::new(main)));
            }
        })?;
        Ok(())
    }

    fn start<R: Runtime>(
        app: &AppHandle<R>,
        details: &CallDetails,
        events: CallEvents,
    ) -> Result<(), CallServiceError> {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let details = details.clone();
        let (done, answer) = mpsc::channel();
        app.run_on_main_thread(move || {
            if let Err(error) = request_start(id, details, events, done.clone()) {
                let _ = done.send(Err(error));
            }
        })
        .map_err(|error| CallServiceError::Unreachable(error.to_string()))?;
        match answer.recv_timeout(START_LIMIT) {
            Ok(result) => result,
            Err(_) => {
                // Not taken in time: it is none of the app's any more, and must not stay on the system's screens.
                let _ = app.run_on_main_thread(move || end(id, None));
                Err(CallServiceError::Unreachable(
                    "CallKit did not answer the call's start".into(),
                ))
            }
        }
    }

    fn update<R: Runtime>(
        app: &AppHandle<R>,
        details: &CallDetails,
    ) -> Result<(), CallServiceError> {
        let details = details.clone();
        app.run_on_main_thread(move || show(details))
            .map_err(|error| CallServiceError::Unreachable(error.to_string()))
    }

    fn stop<R: Runtime>(app: &AppHandle<R>) -> Result<(), CallServiceError> {
        app.run_on_main_thread(|| {
            let id = CALLKIT.with_borrow(|kit| {
                kit.as_ref()
                    .and_then(|kit| kit.call.as_ref())
                    .map(|call| call.id)
            });
            if let Some(id) = id {
                end(id, Some(StopReason::Requested));
            }
        })
        .map_err(|error| CallServiceError::Unreachable(error.to_string()))
    }

    /// CallKit gives the call's audio back by itself (`CallEvent::Audio`); there is no focus to ask for.
    fn refocus<R: Runtime>(_app: &AppHandle<R>) -> Result<(), CallServiceError> {
        Ok(())
    }
}

/// The number of the next start: what a late answer, or a timeout, is told apart by.
static NEXT: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// The app's CallKit objects and its call: on the main thread only.
    static CALLKIT: RefCell<Option<CallKit>> = const { RefCell::new(None) };
}

struct CallKit {
    provider: Retained<CXProvider>,
    controller: Retained<CXCallController>,
    /// Kept: the provider holds its delegate weakly.
    _delegate: Retained<Delegate>,
    call: Option<Call>,
}

/// The call that runs.
struct Call {
    id: u64,
    uuid: Retained<NSUUID>,
    events: CallEvents,
    /// The title the system was last told.
    title: String,
    /// Reported connected (once, when its audio is first activated).
    connected: bool,
    mute: Mute,
}

/// The call's mute, as the page wants it and as CallKit has it.
#[derive(Default)]
struct Mute {
    /// The page's, as last told (or as a mute from outside set it).
    wanted: bool,
    /// CallKit's: what it last performed.
    system: bool,
    /// The mute actions the app asked for and CallKit has neither performed nor refused yet (their `CXAction` UUIDs):
    /// performed, they are the app's own coming back, not a mute from outside, and are not reported (a late one must not
    /// undo a newer mute).
    asked: Vec<Retained<NSUUID>>,
    /// Asked again once already after a refusal, for the mute now wanted.
    retried: bool,
}

impl CallKit {
    fn new(main: MainThreadMarker) -> CallKit {
        // SAFETY: plain property sets on a new configuration, and a provider made from it, on the main thread.
        unsafe {
            let configuration = CXProviderConfiguration::new();
            configuration.setSupportsVideo(false);
            configuration.setMaximumCallGroups(1);
            configuration.setMaximumCallsPerCallGroup(1);
            configuration.setIncludesCallsInRecents(false);
            let generic = NSNumber::new_isize(CXHandleType::Generic.0);
            configuration.setSupportedHandleTypes(&NSSet::from_retained_slice(&[generic]));
            configuration.setIconTemplateImageData(Some(&NSData::with_bytes(ICON)));
            let provider = CXProvider::initWithConfiguration(CXProvider::alloc(), &configuration);
            let delegate = Delegate::new(main);
            // No queue: the delegate is called on the main queue.
            provider.setDelegate_queue(Some(ProtocolObject::from_ref(&*delegate)), None);
            CallKit {
                provider,
                controller: CXCallController::initWithQueue(
                    CXCallController::alloc(),
                    DispatchQueue::main(),
                ),
                _delegate: delegate,
                call: None,
            }
        }
    }
}

/// The handle the system shows for a call on `title`.
fn handle(title: &str) -> Retained<CXHandle> {
    let value = if title.trim().is_empty() {
        NO_CONVERSATION
    } else {
        title.trim()
    };
    // SAFETY: a new handle of a type and a value.
    unsafe {
        CXHandle::initWithType_value(
            CXHandle::alloc(),
            CXHandleType::Generic,
            &NSString::from_str(value),
        )
    }
}

/// Asks CallKit for the call (main thread): `done` gets the answer, once.
fn request_start(
    id: u64,
    details: CallDetails,
    events: CallEvents,
    done: mpsc::Sender<Result<(), CallServiceError>>,
) -> Result<(), CallServiceError> {
    let (controller, action) = CALLKIT.with_borrow_mut(|kit| {
        let kit = kit
            .as_mut()
            .ok_or_else(|| CallServiceError::Unreachable("CallKit is not set up".into()))?;
        if kit.call.is_some() {
            return Err(CallServiceError::Refused {
                code: Some("already-running".into()),
                message: "A call is already running".into(),
            });
        }
        let uuid = NSUUID::new();
        // SAFETY: a new action for a new call.
        let action = unsafe {
            let action = CXStartCallAction::initWithCallUUID_handle(
                CXStartCallAction::alloc(),
                &uuid,
                &handle(&details.title),
            );
            action.setVideo(false);
            action
        };
        kit.call = Some(Call {
            id,
            uuid,
            events,
            title: details.title,
            connected: false,
            mute: Mute::default(),
        });
        Ok((kit.controller.clone(), action))
    })?;
    let answered = RcBlock::new(move |error: *mut NSError| {
        // SAFETY: CallKit hands a valid error, or none, for the length of the call.
        let refused = unsafe { error.as_ref() }.map(|error| {
            let reason = format!(
                "CallKit refused the call ({}, {})",
                error.domain(),
                error.code()
            );
            log::warn!("call: {reason}");
            // The call was never taken: it is not the app's.
            CALLKIT.with_borrow_mut(|kit| {
                if let Some(kit) = kit.as_mut() {
                    if kit.call.as_ref().is_some_and(|call| call.id == id) {
                        kit.call = None;
                    }
                }
            });
            CallServiceError::Refused {
                code: Some("call-refused".into()),
                message: reason,
            }
        });
        let _ = done.send(refused.map_or(Ok(()), Err));
    });
    // SAFETY: the controller answers on the main queue; the block is copied by it.
    unsafe { controller.requestTransactionWithAction_completion(&action, &answered) };
    Ok(())
}

/// Shows the running call as `details` (main thread): its title when it changed, and the page's mute told to the system as
/// the mute action CallKit expects.
fn show(details: CallDetails) {
    let ask = CALLKIT.with_borrow_mut(|kit| {
        let kit = kit.as_mut()?;
        let call = kit.call.as_mut()?;
        if call.title != details.title {
            call.title = details.title.clone();
            // SAFETY: a new update, reported for the call that runs.
            unsafe {
                let update = CXCallUpdate::new();
                update.setRemoteHandle(Some(&handle(&details.title)));
                update.setHasVideo(false);
                kit.provider.reportCallWithUUID_updated(&call.uuid, &update);
            }
        }
        if call.mute.wanted == details.muted {
            return None;
        }
        call.mute.wanted = details.muted;
        call.mute.retried = false;
        Some(details.muted)
    });
    if let Some(muted) = ask {
        ask_mute(muted);
    }
}

/// Asks CallKit to mute or unmute the running call (main thread). A refusal, once nothing else is asked, is asked again
/// once for the mute the page wants then; past that the system's call keeps the mute it has, and the log says so.
fn ask_mute(muted: bool) {
    let request = CALLKIT.with_borrow_mut(|kit| {
        let kit = kit.as_mut()?;
        let call = kit.call.as_mut()?;
        // SAFETY: a new action for the call that runs, and its own identifier.
        let (action, asked) = unsafe {
            let action = CXSetMutedCallAction::initWithCallUUID_muted(
                CXSetMutedCallAction::alloc(),
                &call.uuid,
                muted,
            );
            let asked = action.UUID();
            (action, asked)
        };
        call.mute.asked.push(asked.clone());
        Some((kit.controller.clone(), action, asked, call.id))
    });
    let Some((controller, action, asked, id)) = request else {
        return;
    };
    let answered = RcBlock::new(move |error: *mut NSError| {
        // SAFETY: as above.
        let Some(error) = (unsafe { error.as_ref() }) else {
            return;
        };
        log::warn!(
            "call: CallKit refused the mute ({}, {})",
            error.domain(),
            error.code()
        );
        // Never performed: nothing of it comes back.
        let again = CALLKIT.with_borrow_mut(|kit| {
            let call = kit.as_mut()?.call.as_mut().filter(|call| call.id == id)?;
            call.mute.asked.retain(|uuid| **uuid != *asked);
            let mute = &mut call.mute;
            if !mute.asked.is_empty() || mute.wanted == mute.system {
                return None;
            }
            if mute.retried {
                log::warn!("call: the system's call keeps its mute ({})", mute.system);
                return None;
            }
            mute.retried = true;
            Some(mute.wanted)
        });
        if let Some(muted) = again {
            ask_mute(muted);
        }
    });
    // SAFETY: as in `request_start`.
    unsafe { controller.requestTransactionWithAction_completion(&action, &answered) };
}

/// Ends the call of start `id`, if it is the one that runs (main thread): it is the app's no more, `reason` (if any) is
/// reported, and CallKit is asked to end it.
fn end(id: u64, reason: Option<StopReason>) {
    let ended = CALLKIT.with_borrow_mut(|kit| {
        let kit = kit.as_mut()?;
        if kit.call.as_ref()?.id != id {
            return None;
        }
        let call = kit.call.take()?;
        Some((kit.controller.clone(), call))
    });
    let Some((controller, call)) = ended else {
        return;
    };
    if let Some(reason) = reason {
        (call.events)(CallEvent::Stopped { reason });
    }
    // SAFETY: a new action for the call, which CallKit performs through the delegate (`perform_end` finds no call of the
    // app's and fulfils it).
    unsafe {
        let action = CXEndCallAction::initWithCallUUID(CXEndCallAction::alloc(), &call.uuid);
        let answered = RcBlock::new(|error: *mut NSError| {
            if let Some(error) = error.as_ref() {
                log::warn!(
                    "call: CallKit refused the end ({}, {})",
                    error.domain(),
                    error.code()
                );
            }
        });
        controller.requestTransactionWithAction_completion(&action, &answered);
    }
}

/// The running call, when `uuid` is its, and where its events go (main thread).
fn current(uuid: &NSUUID) -> Option<CallEvents> {
    CALLKIT.with_borrow(|kit| {
        let call = kit.as_ref()?.call.as_ref()?;
        (*call.uuid == *uuid).then(|| call.events.clone())
    })
}

/// The running call's events, whichever it is (main thread).
fn running() -> Option<CallEvents> {
    CALLKIT.with_borrow(|kit| Some(kit.as_ref()?.call.as_ref()?.events.clone()))
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the delegate does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SidevoiceCallProviderDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: each method's signature is the protocol's.
    unsafe impl CXProviderDelegate for Delegate {
        /// The system reset the provider: every call is over.
        #[unsafe(method(providerDidReset:))]
        fn provider_did_reset(&self, _provider: &CXProvider) {
            log::warn!("call: CallKit reset the provider");
            let call = CALLKIT.with_borrow_mut(|kit| kit.as_mut().and_then(|kit| kit.call.take()));
            if let Some(call) = call {
                (call.events)(CallEvent::Stopped {
                    reason: StopReason::System,
                });
            }
        }

        /// The app's own start: connecting from now.
        #[unsafe(method(provider:performStartCallAction:))]
        fn perform_start(&self, provider: &CXProvider, action: &CXStartCallAction) {
            // SAFETY: the action is CallKit's, valid for the call.
            unsafe {
                let uuid = action.callUUID();
                if current(&uuid).is_some() {
                    provider.reportOutgoingCallWithUUID_startedConnectingAtDate(&uuid, None);
                    action.fulfill();
                } else {
                    action.fail();
                }
            }
        }

        /// An end: the app's own (the call is no longer its), or the person's from the system's screens, a headset or a car
        /// kit (it is: hung up).
        #[unsafe(method(provider:performEndCallAction:))]
        fn perform_end(&self, _provider: &CXProvider, action: &CXEndCallAction) {
            // SAFETY: as above.
            let uuid = unsafe { action.callUUID() };
            let id = CALLKIT.with_borrow(|kit| {
                let call = kit.as_ref()?.call.as_ref()?;
                (*call.uuid == *uuid).then_some(call.id)
            });
            if let Some(id) = id {
                let call =
                    CALLKIT.with_borrow_mut(|kit| kit.as_mut().and_then(|kit| kit.call.take()));
                if let Some(call) = call.filter(|call| call.id == id) {
                    (call.events)(CallEvent::Stopped {
                        reason: StopReason::HangUp,
                    });
                }
            }
            // SAFETY: as above.
            unsafe { action.fulfill() };
        }

        /// Muted or unmuted: by the system's screens or a headset (reported), or the app's own request coming back (only
        /// acknowledged: a late one must not undo a newer mute of the page's).
        #[unsafe(method(provider:performSetMutedCallAction:))]
        fn perform_mute(&self, _provider: &CXProvider, action: &CXSetMutedCallAction) {
            // SAFETY: as above.
            let (uuid, muted, id) = unsafe { (action.callUUID(), action.isMuted(), action.UUID()) };
            let external = CALLKIT.with_borrow_mut(|kit| {
                let call = kit.as_mut()?.call.as_mut()?;
                if *call.uuid != *uuid {
                    return None;
                }
                call.mute.system = muted;
                if let Some(at) = call.mute.asked.iter().position(|asked| **asked == *id) {
                    call.mute.asked.remove(at);
                    return None;
                }
                // From outside: the page follows it, so it is what the page wants now.
                call.mute.wanted = muted;
                Some(call.events.clone())
            });
            if let Some(events) = external {
                events(CallEvent::Muted { muted });
            }
            // SAFETY: as above.
            unsafe { action.fulfill() };
        }

        /// Held (another call took this one's place) or taken off hold.
        #[unsafe(method(provider:performSetHeldCallAction:))]
        fn perform_hold(&self, _provider: &CXProvider, action: &CXSetHeldCallAction) {
            // SAFETY: as above.
            let (uuid, held) = unsafe { (action.callUUID(), action.isOnHold()) };
            if let Some(events) = current(&uuid) {
                events(CallEvent::Held { held });
            }
            // SAFETY: as above.
            unsafe { action.fulfill() };
        }

        #[unsafe(method(provider:timedOutPerformingAction:))]
        fn timed_out(&self, _provider: &CXProvider, _action: &CXAction) {
            log::warn!("call: a CallKit action timed out");
        }

        /// The system activated the call's audio session: the app's audio may start. The call is connected from the first.
        #[unsafe(method(provider:didActivateAudioSession:))]
        fn did_activate(&self, provider: &CXProvider, _session: &AVAudioSession) {
            let first = CALLKIT.with_borrow_mut(|kit| {
                let call = kit.as_mut()?.call.as_mut()?;
                let first = !call.connected;
                call.connected = true;
                first.then(|| call.uuid.clone())
            });
            if let Some(uuid) = first {
                // SAFETY: the call that runs.
                unsafe { provider.reportOutgoingCallWithUUID_connectedAtDate(&uuid, None) };
            }
            log::info!("call: CallKit activated the audio session");
            if let Some(events) = running() {
                events(CallEvent::Audio { active: true });
            }
        }

        /// The system deactivated it (the call ended or was held).
        #[unsafe(method(provider:didDeactivateAudioSession:))]
        fn did_deactivate(&self, _provider: &CXProvider, _session: &AVAudioSession) {
            log::info!("call: CallKit deactivated the audio session");
            if let Some(events) = running() {
                events(CallEvent::Audio { active: false });
            }
        }
    }
);

impl Delegate {
    fn new(main: MainThreadMarker) -> Retained<Delegate> {
        let this = Delegate::alloc(main).set_ivars(());
        // SAFETY: NSObject's `init`.
        unsafe { msg_send![super(this), init] }
    }
}
