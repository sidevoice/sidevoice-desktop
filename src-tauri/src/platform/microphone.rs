//! Native microphone access on every OS: the OS privacy policy and whether an input device exists, read fresh on each
//! call. Nothing is cached, no stream is opened, and the webview's own capture outcome plays no part.
use cpal::traits::HostTrait;
use std::{
    future::Future,
    sync::{mpsc, Arc, Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Runtime};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::watch;

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

/// How long a caller waits for the device list before answering `Devices::Unknown`.
const DEADLINE: Duration = Duration::from_secs(2);

/// A host open that fails after this long timed out rather than being refused, and may have left its connection
/// attempt stuck inside the audio library (cpal's PulseAudio handshake gives up after two seconds and abandons its
/// thread).
const STALLED_OPEN: Duration = Duration::from_secs(1);

/// How many audio hosts the app ever opens: the first, and a reopen after each of the first failed listings.
const MAX_HOSTS: usize = 3;

/// Whether an audio input device exists, as seen by this OS's audio host (`Os::audio_host`). Enumeration opens no
/// stream and so never triggers a privacy prompt or a capture indicator.
async fn input_devices() -> Devices {
    static DEVICES: OnceLock<Option<Enumerator>> = OnceLock::new();
    let enumerator = DEVICES.get_or_init(|| {
        Enumerator::start(|jobs| imp::Native::on_worker_thread(|| jobs.serve(scanner::<imp::Native>())))
    });
    match enumerator {
        Some(enumerator) => enumerator.devices(DEADLINE).await,
        None => Devices::Unknown,
    }
}

/// The enumeration the worker runs, on this OS's audio host.
fn scanner<O: Os>() -> impl FnMut() -> Devices {
    reusing_host(O::audio_host, |host: &cpal::Host| {
        host.input_devices().ok().map(|mut devices| devices.any(|device| O::is_capture_source(&device)))
    })
}

/// Enumerate with `list` (`None` when the listing fails) on a host from `open` (`None` when it cannot be opened). The
/// host is opened once and kept, and each request lists afresh on it (the connection is reused, the inventory is never
/// cached). After a failed listing it is dropped and opened again, one attempt at a time since there is one worker.
///
/// Recovery is bounded, and past the bound the answer is `Unknown` until the app restarts. A dropped host can leave its
/// connection behind (cpal's PulseAudio client does not always stop its reactor thread when dropped), so at most
/// `MAX_HOSTS` hosts are ever opened. A host open that stalled is never retried: its stuck attempt cannot be cancelled.
/// An open refused at once (no sound server yet) leaves nothing behind and is retried on the next request.
fn reusing_host<H>(mut open: impl FnMut() -> Option<H>, list: impl Fn(&H) -> Option<bool>) -> impl FnMut() -> Devices {
    let mut host = None;
    let mut opened = 0;
    let mut stalled = false;
    move || {
        if host.is_none() && opened < MAX_HOSTS && !stalled {
            let opening = Instant::now();
            host = open();
            match host {
                Some(_) => opened += 1,
                None => stalled = opening.elapsed() >= STALLED_OPEN,
            }
        }
        // A host that cannot be opened or a listing that fails says nothing about the hardware.
        let Some(open) = &host else { return Devices::Unknown };
        match list(open) {
            Some(present) => present.into(),
            None => {
                host = None;
                Devices::Unknown
            }
        }
    }
}

/// One long-lived thread that owns the audio host and runs one enumeration at a time (on Windows, COM's apartment
/// belongs to the thread, so every call into the host must come from the same one).
///
/// Callers never queue and never wait past their deadline: a caller that arrives while an enumeration runs waits for
/// that one's answer rather than asking for another. No replacement thread is ever started, and connections are
/// reopened only within `reusing_host`'s bound. The residual limit: an enumeration that never returns (a sound server that accepted the connection and stopped answering; cpal
/// cannot cancel a blocked listing) strands this one thread for the life of the app, and every caller from then on
/// answers `Unknown` after its deadline.
struct Enumerator {
    /// Whether an enumeration has been asked for and not yet answered. Guards the hand-off with the worker.
    running: Arc<Mutex<bool>>,
    jobs: mpsc::Sender<()>,
    answers: watch::Receiver<Devices>,
}

/// The worker's end of an `Enumerator`.
struct Jobs {
    running: Arc<Mutex<bool>>,
    jobs: mpsc::Receiver<()>,
    answers: watch::Sender<Devices>,
}

impl Jobs {
    /// Answer each request with a fresh `enumerate`, for as long as the `Enumerator` lives.
    fn serve(self, mut enumerate: impl FnMut() -> Devices) {
        for () in self.jobs {
            let devices = enumerate();
            let mut running = self.running.lock().unwrap();
            *running = false;
            self.answers.send_replace(devices);
        }
    }
}

impl Enumerator {
    /// Start the worker thread, which runs `worker` with its end. `None` when no thread can be started.
    fn start(worker: impl FnOnce(Jobs) + Send + 'static) -> Option<Self> {
        let running = Arc::new(Mutex::new(false));
        let (jobs_tx, jobs) = mpsc::channel();
        let (answers_tx, answers) = watch::channel(Devices::Unknown);
        let end = Jobs { running: running.clone(), jobs, answers: answers_tx };
        thread::Builder::new().name("microphone-devices".into()).spawn(move || worker(end)).ok()?;
        Some(Enumerator { running, jobs: jobs_tx, answers })
    }

    /// The running enumeration's answer, or a new one's when none runs; `Unknown` past `deadline` or when the worker
    /// is gone. The wait is async and holds no runtime thread.
    async fn devices(&self, deadline: Duration) -> Devices {
        let mut answer = {
            let mut running = self.running.lock().unwrap();
            let mut answer = self.answers.clone();
            answer.mark_unchanged();
            if !*running {
                if self.jobs.send(()).is_err() {
                    return Devices::Unknown;
                }
                *running = true;
            }
            answer
        };
        match tokio::time::timeout(deadline, answer.changed()).await {
            Ok(Ok(())) => *answer.borrow(),
            _ => Devices::Unknown,
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

    /// Run `work`, the device worker thread's whole life, after preparing that thread for the audio host's first call
    /// as this OS needs. Nothing to prepare by default.
    fn on_worker_thread(work: impl FnOnce()) {
        work()
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
pub async fn state() -> Result<AccessState, &'static str> {
    state_of::<imp::Native>(input_devices().await)
}

/// Ask for access (see `Os::ask`), then apply the same rule as `state`.
pub async fn request() -> Result<AccessState, &'static str> {
    request_of::<imp::Native>(input_devices().await).await
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
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

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

    /// An enumerator whose fake enumeration answers `Present` only when the test releases it, counting its runs.
    fn held() -> (Enumerator, Arc<AtomicUsize>, mpsc::Sender<()>) {
        let runs = Arc::new(AtomicUsize::new(0));
        let (release, released) = mpsc::channel::<()>();
        let counted = runs.clone();
        let enumerator = Enumerator::start(move |jobs| {
            jobs.serve(|| {
                counted.fetch_add(1, SeqCst);
                released.recv().map_or(Devices::Unknown, |()| Devices::Present)
            })
        })
        .unwrap();
        (enumerator, runs, release)
    }

    const LONG: Duration = Duration::from_secs(30);
    const SHORT: Duration = Duration::from_millis(50);

    #[tokio::test]
    async fn callers_during_an_enumeration_share_its_answer() {
        let (enumerator, runs, release) = held();
        let release = async {
            while runs.load(SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
            release.send(()).unwrap();
        };
        let answers = tokio::join!(enumerator.devices(LONG), enumerator.devices(LONG), release);
        assert_eq!(answers, (Devices::Present, Devices::Present, ()));
        assert_eq!(runs.load(SeqCst), 1);
    }

    #[tokio::test]
    async fn a_stuck_enumeration_answers_unknown_and_queues_nothing_behind_it() {
        let (enumerator, runs, release) = held();
        for _ in 0..5 {
            assert_eq!(enumerator.devices(SHORT).await, Devices::Unknown);
        }
        assert_eq!(runs.load(SeqCst), 1);
        // Once it returns, the next caller gets a fresh enumeration.
        release.send(()).unwrap();
        while *enumerator.running.lock().unwrap() {
            tokio::task::yield_now().await;
        }
        release.send(()).unwrap();
        assert_eq!(enumerator.devices(LONG).await, Devices::Present);
        assert_eq!(runs.load(SeqCst), 2);
    }

    #[test]
    fn a_host_whose_listing_always_fails_is_reopened_a_bounded_number_of_times() {
        let opens = AtomicUsize::new(0);
        let mut scan = reusing_host(|| Some(opens.fetch_add(1, SeqCst)), |_| None);
        for _ in 0..20 {
            assert_eq!(scan(), Devices::Unknown);
        }
        assert_eq!(opens.load(SeqCst), MAX_HOSTS);
    }

    #[test]
    fn an_open_refused_at_once_is_retried_until_one_succeeds() {
        let opens = AtomicUsize::new(0);
        let mut scan = reusing_host(|| (opens.fetch_add(1, SeqCst) >= 5).then_some(()), |()| Some(true));
        for _ in 0..5 {
            assert_eq!(scan(), Devices::Unknown);
        }
        assert_eq!(scan(), Devices::Present);
        assert_eq!(scan(), Devices::Present);
        assert_eq!(opens.load(SeqCst), 6);
    }

    #[tokio::test]
    async fn a_dead_worker_answers_unknown_at_once() {
        let enumerator = Enumerator::start(|jobs| jobs.serve(|| panic!("enumeration failed"))).unwrap();
        assert_eq!(enumerator.devices(LONG).await, Devices::Unknown);
        assert_eq!(enumerator.devices(LONG).await, Devices::Unknown);
    }

    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn state_and_request_answer_without_a_prompt_whatever_the_machine_has() {
        // CI machines have no audio hardware: any answer is fine, a failure or a hang is not.
        let answer = state().await.unwrap();
        assert_eq!(request().await.unwrap(), answer);
        assert_ne!(answer, AccessState::NotDetermined, "only macOS has a prompt to wait for");
    }
}
