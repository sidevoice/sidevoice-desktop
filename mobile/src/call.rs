//! The call's lifecycle on a phone (D-CALL-LIFECYCLE): what the app holds while a call runs, the same on Android and
//! iOS. The call starts when the person presses the green call button (the page starts the voice call) and ends at the
//! red hang-up button (the page stops it); sidevoice-app-core's host tells the app the first start and the last end
//! ([`CallActivity`]).
//!
//! - **Starting**, before the call's audio opens: the audio session is activated (iOS; declared at the app's start,
//!   activated only now, so that opening the app interrupts no other app's audio), then the call's foreground service is
//!   started (Android; iOS keeps the call alive with its `audio` background mode). Either refusing refuses the call,
//!   with the reason, and what was taken is given back.
//! - **Ended**, once the call's audio is closed: the service is stopped and the session deactivated, which lets the
//!   audio another app was playing go on.
//! - **The service ending on its own** (the person pressed Hang up in its notification, removed the app from the recents
//!   list, or the system ended it) hangs up the call on the page, as the red button does.
use sidevoice_app_core::app::CallActivity;
use sidevoice_app_core::sidevoice_voice::{AudioSession, Session, SessionConfig, SessionMode};
use sidevoice_mobile_plugin::{start_call_service, stop_call_service, CallEvent, StopReason};
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

/// The phone's side of the host's calls.
pub struct PhoneCall(pub AppHandle);

impl CallActivity for PhoneCall {
    fn starting(&self) -> Result<(), String> {
        Session::activate()?;
        let app = self.0.clone();
        if let Err(error) = start_call_service(&self.0, move |event| on_service_event(&app, event))
        {
            give_back_session();
            return Err(error.to_string());
        }
        log::info!("call: started");
        Ok(())
    }

    fn ended(&self) {
        if let Err(error) = stop_call_service(&self.0) {
            log::warn!("call: the service did not stop: {error}");
        }
        give_back_session();
        log::info!("call: ended");
    }
}

fn give_back_session() {
    if let Err(error) = Session::deactivate() {
        log::warn!("call: the audio session was not given back: {error}");
    }
}

/// What the call's service reports: an end the app did not ask for hangs the call up; a change of the audio focus is
/// recorded (what to do about it is a decision of its own).
fn on_service_event(app: &AppHandle, event: CallEvent) {
    log::info!("call service: {event:?}");
    if let Some(reason) = hang_up_for(&event) {
        log::info!("call: hung up by the service ({reason:?})");
        run_on_page(app, "hang-up");
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

/// Runs one of the shared commands on the room (`hang-up`, `toggle-mute`, …) through the page's `run`.
pub fn run_on_page(app: &AppHandle, command: &str) {
    if let Some(window) = app.get_webview_window(crate::WINDOW) {
        let request = serde_json::json!({ "command": command });
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
