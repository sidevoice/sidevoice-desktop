//! Native microphone access on every OS: the OS privacy policy and whether an input device exists, read fresh on each
//! call. Nothing is cached, no stream is opened, and the webview's own capture outcome plays no part.
use std::future::Future;
use tauri::{AppHandle, Runtime};
use tauri_plugin_opener::OpenerExt;

#[cfg(target_os = "macos")]
#[path = "microphone/macos.rs"]
mod imp;
#[cfg(target_os = "windows")]
#[path = "microphone/windows.rs"]
mod imp;
#[cfg(target_os = "linux")]
#[path = "microphone/linux.rs"]
mod imp;
pub use imp::NativeMicrophone;

/// Audio input enumeration through the maintained cross-platform audio crate (WASAPI, ALSA); not used on macOS,
/// where AVFoundation answers both questions.
#[cfg(not(target_os = "macos"))]
#[path = "microphone/devices.rs"]
mod devices;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessState {
    Granted,
    Denied,
    /// The OS has not asked the person yet (macOS only).
    NotDetermined,
    NoDevice,
    /// The OS did not answer: never a grant, never a denial, never an absent device.
    Unavailable,
}

/// What the OS says about audio input devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Devices {
    Present,
    Absent,
    /// The device list could not be read. That is not an empty list.
    Unknown,
}

impl From<bool> for Devices {
    fn from(present: bool) -> Self {
        if present {
            Devices::Present
        } else {
            Devices::Absent
        }
    }
}

/// The one rule every OS shares. A denial stands whatever is plugged in (the person fixes it in Settings, whether or
/// not a device is there); a missing device beats everything else; a grant needs a readable device list.
fn resolve(permission: AccessState, devices: Devices) -> AccessState {
    match (permission, devices) {
        (AccessState::Denied, _) => AccessState::Denied,
        (_, Devices::Absent) => AccessState::NoDevice,
        (AccessState::Granted, Devices::Unknown) => AccessState::Unavailable,
        (permission, _) => permission,
    }
}

pub trait MicrophoneAccess {
    /// The fixed system microphone privacy page, where the OS has a standard URI for it.
    const SETTINGS_URL: Option<&'static str> = None;

    /// Inspect without prompting. Re-reads the OS every time. Denial takes precedence over a missing device, and an
    /// OS that cannot be read is `Unavailable`, never a grant.
    fn state() -> Result<AccessState, &'static str>;

    /// Ask for access. Only macOS has a prompt (AVFoundation, the first time; afterwards the stored answer). Elsewhere
    /// there is nothing to ask — Windows' unpackaged desktop apps and Linux have no consent prompt — so this is
    /// `state`. It never edits privacy settings.
    fn request() -> impl Future<Output = Result<AccessState, &'static str>> + Send {
        async { Self::state() }
    }

    /// Open the fixed system microphone privacy page through the registered opener plugin. Opening it proves
    /// nothing about access. UnsupportedPlatform on hosts without a standard settings URI (Linux).
    fn open_settings<R: Runtime>(app: &AppHandle<R>) -> Result<(), tauri_plugin_opener::Error> {
        let url = Self::SETTINGS_URL.ok_or(tauri_plugin_opener::Error::UnsupportedPlatform)?;
        app.opener().open_url(url, None::<&str>)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denial_beats_a_missing_device_and_a_missing_device_beats_the_rest() {
        use AccessState::*;
        use Devices::*;
        for (permission, devices, expected) in [
            (Denied, Present, Denied),
            (Denied, Absent, Denied),
            (Denied, Unknown, Denied),
            (Granted, Present, Granted),
            (Granted, Absent, NoDevice),
            (NotDetermined, Absent, NoDevice),
            (NotDetermined, Present, NotDetermined),
            (Unavailable, Absent, NoDevice),
            (Unavailable, Present, Unavailable),
        ] {
            assert_eq!(resolve(permission, devices), expected, "{permission:?} with {devices:?}");
        }
    }

    #[test]
    fn an_unreadable_device_list_is_neither_a_grant_nor_an_absence() {
        assert_eq!(resolve(AccessState::Granted, Devices::Unknown), AccessState::Unavailable);
        assert_eq!(resolve(AccessState::NotDetermined, Devices::Unknown), AccessState::NotDetermined);
        assert_eq!(Devices::from(true), Devices::Present);
        assert_eq!(Devices::from(false), Devices::Absent);
    }

    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn state_and_request_answer_without_a_prompt_whatever_the_machine_has() {
        // CI machines have no audio hardware: any answer is fine, a failure or a hang is not.
        let state = NativeMicrophone::state().unwrap();
        assert_eq!(NativeMicrophone::request().await.unwrap(), state);
        assert_ne!(state, AccessState::NotDetermined, "only macOS has a prompt to wait for");
    }
}
