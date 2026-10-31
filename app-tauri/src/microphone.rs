//! The microphone command: `host.microphone`'s state, request and open-settings, on every OS. Native policy lives in
//! `platform/microphone`. The room page only (the caller check), like the voice commands.

use serde_json::{json, Value};
use tauri::{AppHandle, Runtime, State, Webview};

use crate::platform::microphone::{self as native, AccessState};
use crate::Callers;

/// A refusal as the page reads it (docs/BRIDGE.md → "Refusals").
fn refusal(key: &str, message: impl std::fmt::Display) -> Value {
    json!({ "key": key, "message": message.to_string() })
}

/// `state`, `request` or `open-settings`: what the system says of the app's microphone access, as one of the page's words
/// (`granted`, `denied`, `prompt`, `missing`), or nothing for opening the system's privacy page.
#[tauri::command]
pub async fn microphone<R: Runtime>(
    app: AppHandle<R>,
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    action: String,
) -> Result<Value, Value> {
    callers.check(&webview).map_err(|why| refusal("bad_request", why))?;
    let state = match action.as_str() {
        "state" => native::state().await,
        "request" => native::request().await,
        "open-settings" => {
            return native::open_settings(&app)
                .map_err(|_| refusal("internal", "System Settings could not be opened."))
                .map(|()| Value::Null)
        }
        _ => return Err(refusal("bad_request", format!("not a microphone action: {action}"))),
    }
    .map_err(|why| refusal("internal", why))?;
    WORDS
        .iter()
        .find(|(known, _)| *known == state)
        .map(|(_, word)| json!(word))
        .ok_or_else(|| refusal("internal", "The system did not answer the microphone request."))
}

/// The page's vocabulary, one word per state. None for an OS that did not answer (`AccessState::Unavailable`), which
/// refuses like any failed read. A table rather than a match: every OS's build names every state the page can read,
/// including those only another OS produces.
const WORDS: [(AccessState, &str); 4] = [
    (AccessState::Granted, "granted"),
    (AccessState::Denied, "denied"),
    (AccessState::NotDetermined, "prompt"),
    (AccessState::NoDevice, "missing"),
];
