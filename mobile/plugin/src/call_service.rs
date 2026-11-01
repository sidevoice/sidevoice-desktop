//! The call's foreground service (Android; docs/MOBILE.md → "The call's service").
//!
//! The service is Kotlin (`android/…/CallService.kt`), started by the app while it is visible and stopped by the app when
//! the call ends. It tells the app what it cannot know otherwise, as [`CallEvent`]s: that it stopped (the person hung up
//! from its notification, removed the app from the recents list, or the system ended it), and that the call lost or
//! regained the audio focus. What to do about either (hang up, mute, resume) is the call's decision, not the service's.
use serde::Deserialize;
use tauri::{ipc::Channel, AppHandle, Runtime};

use crate::platform::call_service as platform;

/// What the service reports.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum CallEvent {
    /// The service ended, once for each start, whoever ended it.
    Stopped { reason: StopReason },
    /// The audio focus the call holds changed.
    Focus { state: FocusState },
}

/// Why the service ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StopReason {
    /// The app asked ([`stop_call_service`]).
    Requested,
    /// The person pressed Hang up in the notification.
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
}

/// Why the service did not start, or could not be asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallServiceError {
    /// The service refused: `code` says why (`not-visible`: the app is not on screen, and Android lets a microphone
    /// service start only from there; `microphone-permission`: the microphone is not allowed yet; `already-running`;
    /// `not-allowed`: the system refused the start).
    Refused {
        code: Option<String>,
        message: String,
    },
    /// The service could not be reached.
    Unreachable(String),
    /// This OS has no such service.
    Unsupported,
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
            CallServiceError::Unsupported => {
                f.write_str("the call's service is not available on this OS")
            }
        }
    }
}

impl std::error::Error for CallServiceError {}

/// Starts the service and reports what happens to it to `on_event`, from any thread. Call it while the app is visible.
/// Where the OS has no service to start (iOS keeps a call alive with a background mode that is configuration), it does
/// nothing and succeeds.
pub fn start_call_service<R: Runtime>(
    app: &AppHandle<R>,
    on_event: impl Fn(CallEvent) + Send + Sync + 'static,
) -> Result<(), CallServiceError> {
    let events = Channel::<CallEvent>::new(move |body| {
        on_event(
            body.deserialize::<CallEvent>()
                .map_err(tauri::Error::Json)?,
        );
        Ok(())
    });
    platform::start(app, events)
}

/// Stops the service, if it runs. It reports `Stopped(Requested)` to the `on_event` it started with.
pub fn stop_call_service<R: Runtime>(app: &AppHandle<R>) -> Result<(), CallServiceError> {
    platform::stop(app)
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
        for (text, state) in [
            ("gained", FocusState::Gained),
            ("lost", FocusState::Lost),
            ("duck", FocusState::Duck),
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
