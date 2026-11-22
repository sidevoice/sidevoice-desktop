//! The call link's commands (`window.__sidevoiceHost.host.callLink`, docs/BRIDGE.md → "The call link"): the call's
//! websocket to the room, opened and kept by the app for the page. Each is the caller check, the arguments and one call of
//! the application host. A link's events go over the channel the page hands `call_link_open`, to that page only.
//!
//! Only the page the app owns the calls for may call these (the capability, and the app's [`crate::init`] caller), and
//! each carries the page's number (`voice_events`), so that a page that was replaced moves nothing of the one after it.

use std::sync::Arc;

use serde_json::Value;
use sidevoice_shared::app::{AppHost, PageGeneration};
use sidevoice_shared::call_link::LinkRequest;
use sidevoice_shared::voice::host::{refusal, Refusal};
use tauri::ipc::Channel;
use tauri::{Runtime, State, Webview};

use crate::Callers;

fn allowed<R: Runtime>(callers: &Callers<R>, webview: &Webview<R>) -> Result<(), Refusal> {
    callers.check(webview).map_err(|why| refusal("bad_request", why))
}

/// Opens the call's socket to `url` with the subprotocols `protocols` (the core's and the device's token) and the page's
/// `language` as `Accept-Language`. Answers at once; the link tells over `events` whether it opened (`open`, or `error`
/// then `close`), the room's frames (`message`, numbered) and how it ended (`close`).
#[tauri::command]
#[allow(clippy::too_many_arguments, reason = "a command's arguments are the page's, one each")]
pub async fn call_link_open<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    key: String,
    url: String,
    protocols: Vec<String>,
    language: Option<String>,
    events: Channel<Value>,
) -> Result<(), Refusal> {
    allowed(&callers, &webview)?;
    let request = LinkRequest { url, protocols, language };
    host.call_link_open(
        page,
        key,
        request,
        Arc::new(move |event: Value| {
            let _ = events.send(event);
        }),
    )
}

/// Sends a text frame on the link, after what the page sent before.
#[tauri::command]
pub async fn call_link_send<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    key: String,
    text: String,
) -> Result<(), Refusal> {
    allowed(&callers, &webview)?;
    host.call_link_send(page, &key, text)
}

/// Closes the link with `code` (1000, a hang-up, by default) and `reason`.
#[tauri::command]
pub async fn call_link_close<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    key: String,
    code: Option<u16>,
    reason: Option<String>,
) -> Result<(), Refusal> {
    allowed(&callers, &webview)?;
    host.call_link_close(page, &key, code, reason)
}

/// What the link holds after the page's last index `after`: `{readyState, frames: [{index, data}], close}`. A page that
/// ran again after it was frozen asks it, since what was sent to it meanwhile may not have reached it.
#[tauri::command]
pub async fn call_link_since<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    key: String,
    after: u64,
) -> Result<Value, Refusal> {
    allowed(&callers, &webview)?;
    host.call_link_since(page, &key, after)
}

/// The page is done with the link (it has read its close): nothing of it is kept.
#[tauri::command]
pub async fn call_link_forget<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    key: String,
) -> Result<(), Refusal> {
    allowed(&callers, &webview)?;
    host.call_link_forget(page, &key)
}
