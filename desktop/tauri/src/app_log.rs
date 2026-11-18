//! The app's own log as the room page reads it (`app_log_read`): pages of whole lines of `sidevoice-desktop.log` and the
//! dated files the log plugin keeps beside it. The work is `sidevoice_desktop_core::app_log`'s; this is the check of who
//! asks and the refusals in the shape of the other room commands (`{key, message}`, docs/BRIDGE.md). A read writes no
//! log line: a viewer of the log would feed itself.

use std::time::Duration;

use serde_json::{json, Value};
use sidevoice_desktop_core::app_log::{read_waiting, Refused};
use tauri::{AppHandle, Webview};

fn caller(app: &AppHandle, webview: &Webview) -> Result<(), Value> {
    crate::room_page(app, webview).map_err(|why| crate::refusal("not_room_page", why))
}

fn refused(error: Refused) -> Value {
    match error {
        Refused::Invalid => crate::refusal("log.request_invalid", "The log request is not valid."),
        Refused::Expired => crate::refusal("log.cursor_expired", "That part of the log is gone."),
        Refused::Unreadable => crate::refusal("log.unreadable", "The log could not be read."),
    }
}

/// `{lines: [{id, text}], cursor, more}`: with no `after` the latest `limit` lines (1 to 1000, default 200), else the
/// lines written after the cursor `after` (the `cursor` of the last page), oldest first; `more` says more is already
/// there. `wait` (seconds, at most 5) holds the answer until a line is written, then answers no line with the cursor
/// unchanged. Refuses `log.request_invalid`, `log.cursor_expired` (the cursor's file is gone: start again without
/// `after`) and `log.unreadable`.
#[tauri::command]
pub async fn app_log_read(
    app: AppHandle,
    webview: Webview,
    after: Option<String>,
    limit: Option<i64>,
    wait: Option<i64>,
) -> Result<Value, Value> {
    caller(&app, &webview)?;
    let invalid = || refused(Refused::Invalid);
    let limit = limit.map(usize::try_from).transpose().map_err(|_| invalid())?;
    let wait = Duration::from_secs(wait.map(u64::try_from).transpose().map_err(|_| invalid())?.unwrap_or(0));
    let active = crate::app_log_file(&app).ok_or_else(|| refused(Refused::Unreadable))?;
    let page = tauri::async_runtime::spawn_blocking(move || read_waiting(&active, after.as_deref(), limit, wait))
        .await
        .map_err(|_| refused(Refused::Unreadable))?
        .map_err(refused)?;
    // The window may have gone elsewhere while the read held on.
    caller(&app, &webview)?;
    Ok(json!(page))
}
