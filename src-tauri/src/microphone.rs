//! Room-only microphone command: `host.microphone`'s state, request and open-settings, on every OS. Native policy
//! lives in platform/microphone.
use crate::{
    platform::microphone::{AccessState, MicrophoneAccess, NativeMicrophone},
    refusal,
};
use serde_json::{json, Value};
use tauri::{AppHandle, Webview};

#[tauri::command]
pub async fn microphone(app: AppHandle, webview: Webview, action: String) -> Result<Value, Value> {
    crate::room_page(&app, &webview).map_err(|why| refusal("bad_request", why))?;
    let state = match action.as_str() {
        "state" => NativeMicrophone::state(),
        "request" => NativeMicrophone::request().await,
        "open-settings" => {
            return NativeMicrophone::open_settings(&app)
                .map_err(|_| refusal("internal", "System Settings could not be opened."))
                .map(|()| Value::Null)
        }
        _ => return Err(refusal("bad_request", format!("not a microphone action: {action}"))),
    }
    .map_err(|why| refusal("internal", why))?;
    // The page's vocabulary: no state of its own for an OS that did not answer, which refuses like any failed read.
    Ok(json!(match state {
        AccessState::Granted => "granted",
        AccessState::Denied => "denied",
        AccessState::NotDetermined => "prompt",
        AccessState::NoDevice => "missing",
        AccessState::Unavailable =>
            return Err(refusal("internal", "The system did not answer the microphone request.")),
    }))
}
