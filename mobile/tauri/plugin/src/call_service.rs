//! The call as the system knows it (docs/MOBILE.md → "The call's service" and "The system's call"): a phone call of the
//! system's own, which the lock screen, the in-call screens and a Bluetooth or car kit show and act on.
//!
//! - **Android**: the call's foreground service (Kotlin, `android/…/CallService.kt`), which keeps the process alive while
//!   the screen is off, and the self-managed call it adds to the system's Telecom (Core-Telecom) for its length.
//! - **iOS**: a CallKit call (`CXProvider`, `CXCallController`), which also keeps the app running, and activates the audio
//!   session for it.
//!
//! The app starts it while it is visible and ends it when the call ends. It tells the app what it cannot know otherwise, as
//! [`CallEvent`]s: that it ended (the person hung up from the system's screens or the notification, removed the app from the
//! recents list, or the system ended it), that the call lost or regained the audio, was muted or held from outside the
//! app, and when the call's audio is ready to open (iOS: CallKit activated the session; Android: Telecom made the call
//! active and settled its route, or the call stays the app's own). What to do about any of it (hang up, mute, pause, resume)
//! is the call's decision, not the service's.
use std::sync::Arc;

use serde::Deserialize;
use tauri::{AppHandle, Runtime};

use crate::call_surface::{end_surface, show_surface, start_surface};
use crate::platform::call_service as platform;
use crate::{CallSurface, SurfaceAction};

/// What the service reports.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum CallEvent {
    /// The service ended, once for each start, whoever ended it.
    Stopped { reason: StopReason },
    /// The audio focus the call holds changed (Android, while the system's Telecom does not hold the call).
    Focus { state: FocusState },
    /// The system's call was muted (`true`) or unmuted from outside the app: its in-call screen, a Bluetooth or car kit.
    Muted { muted: bool },
    /// The system put the call on hold (`true`: another call took its place) or took it off hold.
    Held { held: bool },
    /// The call's audio is ready to open (`true`) or was taken away (see [`call_activates_audio`]). iOS: CallKit activated
    /// or deactivated the session, which it owns while the call lasts. Android: once per start, when Telecom has made the
    /// call active and its route has settled (the communication mode and the loudspeaker, whose change would cut off a
    /// stream already open), or at once when the call stays the app's own (Telecom refused it, or did not answer in time).
    Audio { active: bool },
    /// The person asked for `action` on one of the call's surfaces (the Live Activity's intents, the notification's
    /// buttons, the stack of conversations): `{"type":"action","action":"skip"}`.
    Action { action: SurfaceAction },
}

/// Why the service ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StopReason {
    /// The app asked ([`stop_call_service`]).
    Requested,
    /// The person hung up from outside the app: the notification's Hang up, the system's in-call screen, a headset or a car
    /// kit.
    HangUp,
    /// The person removed the app from the recents list.
    TaskRemoved,
    /// The system ended it, or it could not go on (its start was refused, the process is being torn down).
    System,
}

/// What happened to the call's audio focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FocusState {
    /// Held (again).
    Gained,
    /// Taken for good: another app plays.
    Lost,
    /// Taken for now: a phone call, an alarm. It comes back with `Gained`.
    LostTransient,
    /// Another app may play quietly over the call.
    Duck,
    /// The system refused the call's request outright: no focus is held and none will be given later.
    Refused,
}

/// Why the service did not start, or could not be asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallServiceError {
    /// The service refused: `code` says why (`not-visible`: the app is not on screen, and Android lets a microphone
    /// service start only from there; `microphone-permission`: the microphone is not allowed yet; `already-running`;
    /// `not-allowed`: the system refused the start; `call-refused`: CallKit refused the call).
    Refused {
        code: Option<String>,
        message: String,
    },
    /// The service could not be reached.
    Unreachable(String),
}

impl std::fmt::Display for CallServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallServiceError::Refused {
                code: Some(code),
                message,
            } => write!(f, "{message} ({code})"),
            CallServiceError::Refused {
                code: None,
                message,
            }
            | CallServiceError::Unreachable(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for CallServiceError {}

/// Where a call's events go, from any thread; it must not wait.
pub type CallEvents = Arc<dyn Fn(CallEvent) + Send + Sync>;

/// What the system shows of the call: the conversation it is on (`title`, empty for none), whether the person muted it,
/// and what the call's own surfaces draw (`surface`: the Live Activity, the call's notification).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CallDetails {
    pub title: String,
    pub muted: bool,
    pub surface: CallSurface,
}

/// Starts the call, an outgoing audio call shown as `details`, and reports what happens to it to `on_event`, from any
/// thread. `system`: as a call of the system's (CallKit, Telecom); else it is the app's own, which runs only what the OS
/// keeps for it (Android's foreground service; nothing on iOS, where no CallKit object is made). Call it while the app is
/// visible. A refusal says why, and nothing runs.
///
/// The app's own call also gets the surface the system draws apart from the service (iOS: the call's Live Activity); a
/// system's call does not (D-NO-CALLKIT-DEFAULT: the system draws that call itself, and the two would show it twice).
pub fn start_call_service<R: Runtime>(
    app: &AppHandle<R>,
    details: &CallDetails,
    system: bool,
    on_event: impl Fn(CallEvent) + Send + Sync + 'static,
) -> Result<(), CallServiceError> {
    let events: CallEvents = Arc::new(on_event);
    platform::start(app, details, system, events.clone())?;
    if !system {
        start_surface(details, events);
    }
    Ok(())
}

/// Shows the running call as `details` now: its title, and the mute the page has (the system's screens and a Bluetooth or
/// car kit show it, and on Android the system's microphone mute follows it), and its surface. Called on every change of
/// any of them, at most a few times a second. Nothing when no call runs.
pub fn update_call_service<R: Runtime>(
    app: &AppHandle<R>,
    details: &CallDetails,
) -> Result<(), CallServiceError> {
    show_surface(details);
    platform::update(app, details)
}

/// Ends the system's call, if one runs. It reports `Stopped(Requested)` to the `on_event` it started with.
pub fn stop_call_service<R: Runtime>(app: &AppHandle<R>) -> Result<(), CallServiceError> {
    end_surface();
    platform::stop(app)
}

/// Whether the system makes its call's audio ready, and the app opens its audio only once the service says so
/// ([`CallEvent::Audio`]). iOS: CallKit activates a CallKit call's session, and an app must leave that to it. Android:
/// Telecom making the call active switches the audio mode and the route, which cuts off any stream open before it. False
/// where the app opens its audio when it likes, and for the app's own call (`system` false).
pub fn call_activates_audio(system: bool) -> bool {
    platform::activates_audio(system)
}

/// Whether the system's call takes the place of the app's media controls while it lasts (iOS: the lock screen shows the
/// call, and a headset's buttons act on it through CallKit, so a Now Playing card would be a second one). False where the
/// app keeps its own (Android: Telecom leaves a self-managed call's headset buttons to the app's media session). The app's
/// own call (`system` false) does too where it has a surface of its own (iOS: the Live Activity), so that Now Playing does
/// not show beside it.
pub fn call_replaces_media_controls(system: bool) -> bool {
    platform::replaces_media_controls(system)
}

/// Asks the service to request the audio focus again, after it was lost for good (`FocusState::Lost`) or refused
/// (`FocusState::Refused`). Its answer is a `Focus` event: `Gained`, `LostTransient` (the grant is delayed) or `Refused`
/// (`Gained` at once while the system's Telecom holds the call's audio).
pub fn request_call_focus<R: Runtime>(app: &AppHandle<R>) -> Result<(), CallServiceError> {
    platform::refocus(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(json: &str) -> CallEvent {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn what_the_service_reports_is_read() {
        assert_eq!(
            event(r#"{"type":"stopped","reason":"hang-up"}"#),
            CallEvent::Stopped {
                reason: StopReason::HangUp
            }
        );
        assert_eq!(
            event(r#"{"type":"stopped","reason":"task-removed"}"#),
            CallEvent::Stopped {
                reason: StopReason::TaskRemoved
            }
        );
        assert_eq!(
            event(r#"{"type":"stopped","reason":"system"}"#),
            CallEvent::Stopped {
                reason: StopReason::System
            }
        );
        assert_eq!(
            event(r#"{"type":"stopped","reason":"requested"}"#),
            CallEvent::Stopped {
                reason: StopReason::Requested
            }
        );
        assert_eq!(
            event(r#"{"type":"focus","state":"lost-transient"}"#),
            CallEvent::Focus {
                state: FocusState::LostTransient
            }
        );
        assert_eq!(
            event(r#"{"type":"muted","muted":true}"#),
            CallEvent::Muted { muted: true }
        );
        assert_eq!(
            event(r#"{"type":"held","held":false}"#),
            CallEvent::Held { held: false }
        );
        assert_eq!(
            event(r#"{"type":"audio","active":true}"#),
            CallEvent::Audio { active: true }
        );
        assert_eq!(
            event(r#"{"type":"action","action":"cancel-send"}"#),
            CallEvent::Action {
                action: SurfaceAction::CancelSend
            }
        );
        for (text, state) in [
            ("gained", FocusState::Gained),
            ("lost", FocusState::Lost),
            ("duck", FocusState::Duck),
            ("refused", FocusState::Refused),
        ] {
            assert_eq!(
                event(&format!(r#"{{"type":"focus","state":"{text}"}}"#)),
                CallEvent::Focus { state }
            );
        }
    }

    #[test]
    fn what_it_does_not_say_is_an_error() {
        assert!(
            serde_json::from_str::<CallEvent>(r#"{"type":"stopped","reason":"nope"}"#).is_err()
        );
        assert!(serde_json::from_str::<CallEvent>(r#"{"type":"ringing"}"#).is_err());
        assert!(
            serde_json::from_str::<CallEvent>(r#"{"type":"action","action":"dance"}"#).is_err()
        );
    }

    #[test]
    fn a_refusal_says_why() {
        let refused = CallServiceError::Refused {
            code: Some("not-visible".into()),
            message: "not on screen".into(),
        };
        assert_eq!(refused.to_string(), "not on screen (not-visible)");
        assert_eq!(
            CallServiceError::Unreachable("gone".into()).to_string(),
            "gone"
        );
    }
}
