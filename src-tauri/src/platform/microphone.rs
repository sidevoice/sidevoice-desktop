//! Native microphone access on every OS: the OS privacy policy and whether an input device exists, read fresh on each
//! call. Nothing is cached, no stream is opened, and the webview's own capture outcome plays no part.
use cpal::traits::HostTrait;
use std::{
    future::Future,
    sync::{mpsc, OnceLock},
    thread,
};
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

/// Whether an audio input device exists, as seen by this OS's audio host (`Os::audio_host`). Enumeration opens no
/// stream and so never triggers a privacy prompt or a capture indicator.
///
/// It always runs on one dedicated long-lived thread, started on first use and parked on its channel in between:
/// WASAPI's enumerator needs COM initialised on the calling thread, and cpal initialises it only on the thread that
/// first enumerates, so every later enumeration must come from that same thread. The caller waits for the answer.
fn input_devices() -> Devices {
    static WORKER: OnceLock<Option<mpsc::Sender<mpsc::Sender<Devices>>>> = OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (jobs_tx, jobs) = mpsc::channel::<mpsc::Sender<Devices>>();
        thread::Builder::new()
            .name("microphone-devices".into())
            .spawn(move || {
                for reply in jobs {
                    let _ = reply.send(scan::<imp::Native>());
                }
            })
            .ok()
            .map(|_| jobs_tx)
    });
    let Some(worker) = worker else { return Devices::Unknown };
    let (reply, answer) = mpsc::channel();
    worker.send(reply).ok().and_then(|()| answer.recv().ok()).unwrap_or(Devices::Unknown)
}

fn scan<O: Os>() -> Devices {
    // A host that cannot be opened or a listing that fails says nothing about the hardware.
    let Some(host) = O::audio_host() else { return Devices::Unknown };
    match host.input_devices() {
        Ok(mut devices) => devices.any(|device| O::is_capture_source(&device)).into(),
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

    /// The audio host whose input devices count as microphones; `None` when it cannot be opened, which says nothing
    /// about the hardware (`Devices::Unknown`). The platform default wherever that is trustworthy.
    fn audio_host() -> Option<cpal::Host> {
        Some(cpal::default_host())
    }

    /// Whether an input device the host lists is a real capture source rather than, say, the monitor of an output.
    fn is_capture_source(_device: &cpal::Device) -> bool {
        true
    }

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

    #[test]
    fn enumeration_from_any_thread_goes_through_the_one_worker() {
        let answers: Vec<_> = (0..4).map(|_| thread::spawn(input_devices)).collect();
        let answers: Vec<_> = answers.into_iter().map(|answer| answer.join().unwrap()).collect();
        assert!(answers.iter().all(|answer| *answer == input_devices()));
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
