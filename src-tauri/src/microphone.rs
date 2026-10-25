//! Room-only microphone commands. Native permission policy lives in platform/microphone.
use crate::{
    platform::microphone::{AccessState, BackendObservation, MicrophoneAccess, NativeMicrophone},
    refusal,
};
use serde_json::{json, Value};
use tauri::{AppHandle, Webview};

#[tauri::command]
pub async fn microphone(
    app: AppHandle,
    webview: Webview,
    action: String,
    backend: Option<BackendObservation>,
) -> Result<Value, Value> {
    crate::room_page(&app, &webview).map_err(|why| refusal("bad_request", why))?;
    let backend = backend.unwrap_or_default();
    let state = match action.as_str() {
        "state" | "access-state" => NativeMicrophone::state(backend),
        "request" | "access-request" => NativeMicrophone::request(backend).await,
        "open-settings" => {
            return NativeMicrophone::open_settings(&app)
                .map_err(|_| refusal("internal", "System Settings could not be opened."))
                .map(|()| Value::Null)
        }
        _ => return Err(refusal("bad_request", format!("not a microphone action: {action}"))),
    }
    .map_err(|why| refusal("internal", why))?;
    if action.starts_with("access-") {
        return Ok(json!(state));
    }
    // Preserve the original macOS bridge vocabulary and refusal contract.
    Ok(json!(match state {
        AccessState::Granted => "granted",
        AccessState::Denied => "denied",
        AccessState::NotDetermined => "prompt",
        AccessState::NoDevice => "missing",
        AccessState::Unavailable =>
            return Err(refusal("internal", "The system did not answer the microphone request.")),
    }))
}
