//! AVFoundation's microphone permission, the same API and the same prompt on macOS and iOS: the status, and the
//! one-shot request callback. What each of the two adds (the privacy page it opens) is in its own file.
use super::AccessState;
use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaType, AVMediaTypeAudio};
use std::sync::Mutex;
use tokio::sync::oneshot;

/// The permission as the page reads it, without prompting.
pub fn permission() -> Result<AccessState, &'static str> {
    Ok(access(authorization()?))
}

/// Asks for the microphone: the first time the OS prompts, afterwards it answers at once with the stored one.
pub async fn ask() -> Result<AccessState, &'static str> {
    let granted = request_access()?.await.map_err(|_| "The system did not answer the microphone request.")?;
    Ok(if granted { AccessState::Granted } else { AccessState::Denied })
}

fn access(status: AVAuthorizationStatus) -> AccessState {
    match status {
        AVAuthorizationStatus::NotDetermined => AccessState::NotDetermined,
        AVAuthorizationStatus::Authorized => AccessState::Granted,
        // Denied, Restricted (a profile or parental controls forbid it), and any status a later OS adds: the
        // page cannot get the microphone by asking.
        _ => AccessState::Denied,
    }
}

fn audio() -> Result<&'static AVMediaType, &'static str> {
    // SAFETY: an immutable constant AVFoundation exports.
    unsafe { AVMediaTypeAudio }.ok_or("AVMediaTypeAudio is missing.")
}

fn authorization() -> Result<AVAuthorizationStatus, &'static str> {
    let audio = audio()?;
    // SAFETY: AVMediaTypeAudio is one of the two media types the method accepts.
    Ok(unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) })
}

/// Asks for the microphone; the answer arrives on the receiver. AVFoundation calls the handler once, on an arbitrary
/// queue: at once with the stored answer when there is one, else when the person answers the system's prompt. Kept out of
/// the command's future, whose state must be `Send` (the block is not).
fn request_access() -> Result<oneshot::Receiver<bool>, &'static str> {
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
