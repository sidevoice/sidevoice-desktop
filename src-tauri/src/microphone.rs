//! The microphone permission for the room page (`window.__sidevoiceDesktop.host.microphone`, docs/BRIDGE.md → "The
//! microphone permission"): where the app runs the voice call itself, the page is never granted `getUserMedia`
//! (core `media::decide`), so it can neither ask macOS for the microphone nor read whether it may. This asks and reads
//! for it, on the app's own privacy entry (TCC, with `NSMicrophoneUsageDescription`), which is the one the voice call
//! uses: the page's first-run setup asks early instead of at the first call.
//!
//! - `state`: `AVCaptureDevice.authorizationStatus(for: .audio)`: `prompt` (not determined), `granted`, `denied`
//!   (denied or restricted). Never prompts.
//! - `request`: `AVCaptureDevice.requestAccess(for: .audio)`: macOS's own prompt the first time; afterwards the stored
//!   answer at once, with no prompt.
//! - `open-settings`: System Settings at Privacy & Security › Microphone, where a denial is undone.
//!
//! Only the room window's own page may call it (capabilities/room.json, and `room_page`). macOS only, as `host.voice`
//! (min macOS 13, so not AVAudioApplication's record permission, which is 14+).
#![cfg(target_os = "macos")]

use std::sync::Mutex;

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaType, AVMediaTypeAudio};
use serde_json::{json, Value};
use tauri::{AppHandle, Webview};
use tokio::sync::oneshot;

use crate::refusal;

/// System Settings → Privacy & Security → Microphone.
const SETTINGS_URL: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";

/// `host.microphone`'s one command: `action` `state`, `request` or `open-settings`. `state` and `request` answer the
/// permission as the page reads it (`"prompt" | "granted" | "denied"`; `request` never `"prompt"`), `open-settings`
/// nothing. Refuses `bad_request` for another action or caller, `internal` when the system does not answer.
#[tauri::command]
pub async fn microphone(app: AppHandle, webview: Webview, action: String) -> Result<Value, Value> {
    crate::room_page(&app, &webview).map_err(|why| refusal("bad_request", why))?;
    match action.as_str() {
        "state" => Ok(json!(permission(authorization()?))),
        "request" => {
            let granted = request_access()?
                .await
                .map_err(|_| refusal("internal", "The system did not answer the microphone request."))?;
            Ok(json!(if granted { "granted" } else { "denied" }))
        }
        "open-settings" => open_settings().await.map(|()| Value::Null),
        _ => Err(refusal("bad_request", format!("not a microphone action: {action}"))),
    }
}

/// The permission as the page reads it.
fn permission(status: AVAuthorizationStatus) -> &'static str {
    match status {
        AVAuthorizationStatus::NotDetermined => "prompt",
        AVAuthorizationStatus::Authorized => "granted",
        // Denied, Restricted (a profile or parental controls forbid it), and any status a later macOS adds: the
        // page cannot get the microphone by asking.
        _ => "denied",
    }
}

fn audio() -> Result<&'static AVMediaType, Value> {
    // SAFETY: an immutable constant AVFoundation exports.
    unsafe { AVMediaTypeAudio }.ok_or_else(|| refusal("internal", "AVMediaTypeAudio is missing."))
}

fn authorization() -> Result<AVAuthorizationStatus, Value> {
    let audio = audio()?;
    // SAFETY: AVMediaTypeAudio is one of the two media types the method accepts.
    Ok(unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) })
}

/// Asks for the microphone; the answer arrives on the receiver. AVFoundation calls the handler once, on an arbitrary
/// queue: at once with the stored answer when there is one, else when the person answers macOS's prompt. Kept out of
/// the command's future, whose state must be `Send` (the block is not).
fn request_access() -> Result<oneshot::Receiver<bool>, Value> {
    let audio = audio()?;
    let (tx, rx) = oneshot::channel();
    let tx = Mutex::new(Some(tx));
    let handler = RcBlock::new(move |granted: Bool| {
        if let Some(tx) = tx.lock().unwrap().take() {
            let _ = tx.send(granted.as_bool());
        }
    });
    // SAFETY: AVMediaTypeAudio is one of the two media types the method accepts; AVFoundation copies the block.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &handler) };
    Ok(rx)
}

/// System Settings at the microphone's privacy pane (`open <url>`, no shell).
async fn open_settings() -> Result<(), Value> {
    let open = || std::process::Command::new("/usr/bin/open").arg(SETTINGS_URL).status();
    let opened = tauri::async_runtime::spawn_blocking(open).await;
    match opened {
        Ok(Ok(status)) if status.success() => Ok(()),
        _ => Err(refusal("internal", "System Settings could not be opened.")),
    }
}
