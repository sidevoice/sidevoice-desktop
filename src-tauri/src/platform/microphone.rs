//! Native microphone access on every OS: the OS privacy policy and whether an input device exists, read fresh on each
//! call. Nothing is cached, no stream is opened, and the webview's own capture outcome plays no part.
use cpal::traits::HostTrait;
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

/// Whether an audio input device exists, from cpal's host enumeration (WASAPI active endpoints, CoreAudio, ALSA PCM
/// hints). Enumeration opens no stream and so never triggers a privacy prompt or a capture indicator.
fn input_devices() -> Devices {
    let Some(id) = cpal::available_hosts().into_iter().next() else { return Devices::Unknown };
    // A host or a listing that fails says nothing about the hardware.
    match cpal::host_from_id(id).and_then(|host| host.input_devices()) {
        Ok(mut devices) => devices.next().is_some().into(),
        Err(_) => Devices::Unknown,
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

/// What each OS says, and nothing else: the shared rule and the device list are applied once, below.
trait Os {
    /// The fixed system microphone privacy page, where the OS has a standard URI for it.
    const SETTINGS_URL: Option<&'static str> = None;

    /// The OS's permission alone, re-read every time and without prompting. An OS that cannot be read is
    /// `Unavailable`, never a grant.
    fn permission() -> Result<AccessState, &'static str>;

    /// Ask for access. Only macOS has a prompt (AVFoundation, the first time; afterwards the stored answer). Elsewhere
    /// there is nothing to ask — Windows' unpackaged desktop apps and Linux have no consent prompt — so this is
    /// `permission`. It never edits privacy settings.
    fn ask() -> impl Future<Output = Result<AccessState, &'static str>> + Send {
        async { Self::permission() }
    }
}

fn state_of<O: Os>(devices: Devices) -> Result<AccessState, &'static str> {
    Ok(resolve(O::permission()?, devices))
}

async fn request_of<O: Os>(devices: Devices) -> Result<AccessState, &'static str> {
    Ok(resolve(O::ask().await?, devices))
}

/// Inspect without prompting. Re-reads the OS every time. Denial takes precedence over a missing device, and an OS
/// that cannot be read is `Unavailable`, never a grant.
pub fn state() -> Result<AccessState, &'static str> {
    state_of::<imp::Native>(input_devices())
}

/// Ask for access (see `Os::ask`), then apply the same rule as `state`.
pub async fn request() -> Result<AccessState, &'static str> {
    request_of::<imp::Native>(input_devices()).await
}

/// Whether this OS has a fixed microphone privacy page to open.
pub fn settings_available() -> bool {
    imp::Native::SETTINGS_URL.is_some()
}

/// Open the fixed system microphone privacy page through the registered opener plugin. Opening it proves nothing
/// about access. UnsupportedPlatform on hosts without a standard settings URI (Linux).
pub fn open_settings<R: Runtime>(app: &AppHandle<R>) -> Result<(), tauri_plugin_opener::Error> {
    let url = imp::Native::SETTINGS_URL.ok_or(tauri_plugin_opener::Error::UnsupportedPlatform)?;
    app.opener().open_url(url, None::<&str>)
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

    struct Allowed;
    impl Os for Allowed {
        fn permission() -> Result<AccessState, &'static str> {
            Ok(AccessState::Granted)
        }
    }

    struct Refused;
    impl Os for Refused {
        const SETTINGS_URL: Option<&'static str> = Some("settings:microphone");
        fn permission() -> Result<AccessState, &'static str> {
            Ok(AccessState::Denied)
        }
    }

    struct Broken;
    impl Os for Broken {
        fn permission() -> Result<AccessState, &'static str> {
            Err("unreadable")
        }
    }

    #[tokio::test]
    async fn an_os_only_supplies_its_permission_and_the_shared_rule_does_the_rest() {
        assert_eq!(state_of::<Allowed>(Devices::Present), Ok(AccessState::Granted));
        assert_eq!(state_of::<Allowed>(Devices::Absent), Ok(AccessState::NoDevice));
        assert_eq!(state_of::<Refused>(Devices::Absent), Ok(AccessState::Denied));
        assert_eq!(state_of::<Broken>(Devices::Present), Err("unreadable"));
        // Without an override, asking is reading.
        assert_eq!(request_of::<Allowed>(Devices::Unknown).await, Ok(AccessState::Unavailable));
        assert_eq!(request_of::<Refused>(Devices::Present).await, Ok(AccessState::Denied));
        assert_eq!(request_of::<Broken>(Devices::Present).await, Err("unreadable"));
        assert_eq!(Allowed::SETTINGS_URL, None);
        assert!(Refused::SETTINGS_URL.is_some());
    }

    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn state_and_request_answer_without_a_prompt_whatever_the_machine_has() {
        // CI machines have no audio hardware: any answer is fine, a failure or a hang is not.
        let answer = state().unwrap();
        assert_eq!(request().await.unwrap(), answer);
        assert_ne!(answer, AccessState::NotDetermined, "only macOS has a prompt to wait for");
    }
}
