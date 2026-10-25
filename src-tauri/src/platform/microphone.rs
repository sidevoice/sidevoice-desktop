//! Microphone privacy policy, combined with observations from the backend that actually captures audio.
//! Windows/Linux currently capture in WebView2/WebKit; do not open a second native audio backend here.
use serde::{Deserialize, Serialize};
use std::future::Future;
use tauri::{AppHandle, Runtime};
use tauri_plugin_opener::OpenerExt;

#[cfg(target_os = "macos")]
#[path = "microphone/macos.rs"]
mod imp;
#[cfg(target_os = "windows")]
#[path = "microphone/windows.rs"]
mod imp;
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use imp::NativeMicrophone;

#[cfg(target_os = "macos")]
pub const SETTINGS_URL: Option<&str> =
    Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone");
#[cfg(target_os = "windows")]
pub const SETTINGS_URL: Option<&str> = Some("ms-settings:privacy-microphone");
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const SETTINGS_URL: Option<&str> = None;

#[derive(Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccessState {
    Granted,
    Denied,
    NotDetermined,
    NoDevice,
    #[default]
    Unavailable,
}

/// A fresh observation from the current capture backend, never a cached authorization or an access grant.
/// `error` is the DOMException name from WebView2/WebKit getUserMedia (Linux's session audio backend).
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendObservation {
    #[serde(default)]
    pub permission: AccessState,
    pub has_device: Option<bool>,
    pub error: Option<String>,
    /// True only when this operation opened a live audio track (not merely a permission query).
    #[serde(default)]
    pub captured: bool,
}

impl BackendObservation {
    fn state(self) -> AccessState {
        let permission = match self.error.as_deref() {
            Some("NotAllowedError" | "PermissionDeniedError" | "SecurityError") => AccessState::Denied,
            Some("NotFoundError" | "DevicesNotFoundError") => AccessState::NoDevice,
            // Busy/broken backends, constraints, interruption, and unknown errors do not establish a denial
            // or prove that the computer has no device (e.g. PipeWire/Pulse not available to WebKit).
            Some(_) => AccessState::Unavailable,
            None if self.captured => AccessState::Granted,
            None => self.permission,
        };
        if permission != AccessState::Denied && self.has_device == Some(false) {
            AccessState::NoDevice
        } else {
            permission
        }
    }
}

// Unknown registry keys must not turn a browser permission hint into an OS grant. A successful capture
// is stronger evidence; it still cannot override an explicit privacy denial read in this operation.
#[cfg(any(target_os = "windows", test))]
fn privacy_state(backend: BackendObservation, values: [Option<String>; 2]) -> AccessState {
    if values.iter().any(|value| value.as_deref() == Some("Deny")) {
        return AccessState::Denied;
    }
    let captured = backend.captured;
    let state = backend.state();
    if state == AccessState::Granted && !captured && values.iter().any(|value| value.as_deref() != Some("Allow")) {
        AccessState::Unavailable
    } else {
        state
    }
}

pub trait MicrophoneAccess {
    /// Inspect without prompting. Re-read OS privacy policy every time; combine it with a fresh capture/backend
    /// observation. Denial takes precedence over absence. An unreadable backend never means granted.
    /// macOS supplies its own AVFoundation permission and device observation, ignoring `backend`.
    fn state(backend: BackendObservation) -> Result<AccessState, &'static str> {
        Ok(backend.state())
    }

    /// Request access: macOS uses its existing AVFoundation prompt. On hosts whose capture belongs to the
    /// webview, the caller first requests getUserMedia and supplies its outcome; this applies current OS policy.
    /// This never edits privacy settings, and a desktop app cannot enable Windows' desktop-app switch itself.
    fn request(backend: BackendObservation) -> impl Future<Output = Result<AccessState, &'static str>> + Send {
        async { Self::state(backend) }
    }

    /// Open the fixed system microphone privacy page through the registered opener plugin. Opening it proves
    /// nothing about access. UnsupportedPlatform on hosts without a standard settings URI (Linux).
    fn open_settings<R: Runtime>(app: &AppHandle<R>) -> Result<(), tauri_plugin_opener::Error> {
        let url = SETTINGS_URL.ok_or(tauri_plugin_opener::Error::UnsupportedPlatform)?;
        app.opener().open_url(url, None::<&str>)
    }
}

// Linux has no independent TCC-style prompt: WebKit's existing capture backend supplies the outcome.
// No empty OS file and no second PipeWire/Pulse connection or portal.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub struct NativeMicrophone;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl MicrophoneAccess for NativeMicrophone {}

#[cfg(test)]
mod tests {
    use super::*;

    struct BackendMicrophone;
    impl MicrophoneAccess for BackendMicrophone {}

    fn observation(permission: AccessState, has_device: Option<bool>) -> BackendObservation {
        BackendObservation { permission, has_device, ..Default::default() }
    }

    #[tokio::test]
    async fn backend_denial_absence_and_changes_are_fresh_observations() {
        assert_eq!(BackendMicrophone::state(BackendObservation::default()).unwrap(), AccessState::Unavailable);
        for (permission, device, expected) in [
            (AccessState::Denied, Some(false), AccessState::Denied),
            (AccessState::Granted, Some(false), AccessState::NoDevice),
            (AccessState::NotDetermined, Some(true), AccessState::NotDetermined),
            (AccessState::Granted, Some(true), AccessState::Granted),
            (AccessState::Denied, Some(true), AccessState::Denied),
            (AccessState::Granted, Some(true), AccessState::Granted),
        ] {
            assert_eq!(BackendMicrophone::request(observation(permission, device)).await.unwrap(), expected);
        }
    }

    #[test]
    fn capture_errors_distinguish_denial_missing_and_backend_failure() {
        for (error, expected) in [
            ("NotAllowedError", AccessState::Denied),
            ("PermissionDeniedError", AccessState::Denied),
            ("SecurityError", AccessState::Denied),
            ("NotFoundError", AccessState::NoDevice),
            ("DevicesNotFoundError", AccessState::NoDevice),
            ("NotReadableError", AccessState::Unavailable),
            ("TrackStartError", AccessState::Unavailable),
            ("OverconstrainedError", AccessState::Unavailable),
            ("AbortError", AccessState::Unavailable),
            ("new backend failure", AccessState::Unavailable),
        ] {
            assert_eq!(
                BackendMicrophone::state(BackendObservation { error: Some(error.into()), ..Default::default() })
                    .unwrap(),
                expected,
                "{error}"
            );
        }
    }

    #[test]
    fn privacy_changes_override_backend_hints_and_unknown_is_not_a_grant() {
        let allowed = || Some("Allow".into());
        let denied = || Some("Deny".into());
        let query = || observation(AccessState::Granted, Some(true));
        assert_eq!(privacy_state(query(), [allowed(), allowed()]), AccessState::Granted);
        assert_eq!(privacy_state(query(), [allowed(), denied()]), AccessState::Denied);
        assert_eq!(privacy_state(query(), [denied(), allowed()]), AccessState::Denied);
        assert_eq!(privacy_state(query(), [allowed(), allowed()]), AccessState::Granted);
        assert_eq!(privacy_state(query(), [None, allowed()]), AccessState::Unavailable);
        assert_eq!(privacy_state(query(), [allowed(), Some("future value".into())]), AccessState::Unavailable);
        assert_eq!(
            privacy_state(
                BackendObservation { captured: true, has_device: Some(true), ..Default::default() },
                [None, None]
            ),
            AccessState::Granted
        );
        assert_eq!(
            privacy_state(
                BackendObservation { captured: true, has_device: Some(true), ..Default::default() },
                [None, denied()]
            ),
            AccessState::Denied
        );
        assert_eq!(
            privacy_state(observation(AccessState::Granted, Some(false)), [allowed(), denied()]),
            AccessState::Denied
        );
    }

    #[test]
    fn native_inspection_without_backend_evidence_never_assumes_granted() {
        // macOS has its own device/authorization inspection. Other hosts need a capture observation.
        let state = NativeMicrophone::state(BackendObservation::default()).unwrap();
        if !cfg!(target_os = "macos") {
            assert!(matches!(state, AccessState::Unavailable | AccessState::Denied));
        }
    }
}
