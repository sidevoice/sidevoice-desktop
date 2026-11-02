//! The pairings the room page keeps (the machines it is paired with and the revocations still to tell them): read and
//! written whole, as the page's JSON. Where and how they are kept is the app's, through [`PairingStore`]: the desktop's is
//! a private file in a held directory; a phone's is its own private storage.

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::{Runtime, State, Webview};

use crate::Callers;

/// Where the app keeps the pairings. A refusal is the page's: `{key, …}` (docs/BRIDGE.md → "Refusals").
pub trait PairingStore: Send + Sync + 'static {
    /// The pairings as the page wrote them last, or the empty set when none were.
    fn read(&self) -> Result<Value, Value>;

    /// Keeps `value` after checking its shape; refuses what is not pairings, changing nothing.
    fn write(&self, value: &Value) -> Result<Value, Value>;
}

/// The app's store, as the plugin's state: the app manages one.
pub struct Pairings(pub Arc<dyn PairingStore>);

/// A caller that is not the room page.
fn unauthorized(why: String) -> Value {
    json!({ "key": "bad_request", "message": why })
}

/// The pairings the page kept.
#[tauri::command]
pub fn pairings_read<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    pairings: State<'_, Pairings>,
) -> Result<Value, Value> {
    callers.check(&webview).map_err(unauthorized)?;
    pairings.0.read()
}

/// Keeps the pairings the page sends.
#[tauri::command]
pub fn pairings_write<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    pairings: State<'_, Pairings>,
    value: Value,
) -> Result<Value, Value> {
    callers.check(&webview).map_err(unauthorized)?;
    pairings.0.write(&value)
}
