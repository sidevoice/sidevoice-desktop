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
use std::sync::Mutex;

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

/// The phone's side of the host's voice calls: the audio session while any of them has audio open.
pub struct PhoneCall;

impl CallActivity for PhoneCall {
    fn starting(&self) -> Result<(), String> {
        Session::activate()
    }

    fn ended(&self) {
        if let Err(error) = Session::deactivate() {
            log::warn!("call: the audio session was not given back: {error}");
        }
    }
}

/// Whether the room's call holds the call's service now.
#[derive(Default)]
pub struct RoomCall(Mutex<bool>);

impl RoomCall {
    /// The page says whether the call is joined: the service follows, starting at the join and stopping at the end.
    pub fn joined(&self, app: &AppHandle, joined: bool) {
        let mut holding = self.0.lock().unwrap();
        if joined == *holding {
            return;
        }
        if joined {
            let events = app.clone();
            match start_call_service(app, move |event| on_service_event(&events, event)) {
                Ok(()) => {
                    *holding = true;
                    log::info!("call: started");
                }
                // The call goes on in front; it does not outlive the screen. Recorded by its code
                // (`not-visible`, `microphone-permission`, …).
                Err(error) => log::error!("call: the call's service did not start: {error}"),
            }
        } else {
            *holding = false;
            if let Err(error) = stop_call_service(app) {
                log::warn!("call: the call's service did not stop: {error}");
            }
            log::info!("call: ended");
        }
    }
}

/// What the call's service reports: an end the app did not ask for hangs the call up; a change of the audio focus is
/// recorded (what to do about it is a decision of its own).
fn on_service_event(app: &AppHandle, event: CallEvent) {
    log::info!("call service: {event:?}");
    if let Some(reason) = hang_up_for(&event) {
        log::info!("call: hung up by the service ({reason:?})");
        // The service is gone: the next join starts another.
        if let Some(room) = app.try_state::<RoomCall>() {
            *room.0.lock().unwrap() = false;
        }
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
