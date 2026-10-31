//! Provider secrets in the OS credential store (docs/CREDENTIALS.md). No store error or secret is formatted or logged.
//! The service/account pair is the app's namespace (the `service`, which the app passes in exactly), not a
//! catalog-dependent list.
//!
//! The common code here owns the naming, validation, requests, namespace clearing, admission and error categories.
//! What differs per OS is the store itself (`Os::store`), how a whole namespace is deleted (`Os::clear_namespace`) and
//! whether store calls can wait on a prompt (`Os::PROMPTS`).

use keyring_core::{CredentialStore, Entry};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};

#[cfg_attr(target_os = "macos", path = "credentials/macos.rs")]
#[cfg_attr(target_os = "windows", path = "credentials/windows.rs")]
#[cfg_attr(target_os = "linux", path = "credentials/linux.rs")]
#[cfg_attr(target_os = "ios", path = "credentials/ios.rs")]
#[cfg_attr(target_os = "android", path = "credentials/android.rs")]
mod imp;
use imp::Native;

/// How long a caller waits for a store that can prompt (`Os::PROMPTS`) before answering `Unavailable`.
const WAIT: Duration = Duration::from_secs(30);

/// Safe error categories only: keyring errors can contain secret bytes or backend-supplied text.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    InvalidProvider,
    TooLong,
    Failed,
}

impl From<keyring_core::Error> for Error {
    fn from(error: keyring_core::Error) -> Self {
        match error {
            keyring_core::Error::NoStorageAccess(_) | keyring_core::Error::PlatformFailure(_) => Self::Unavailable,
            keyring_core::Error::TooLong(_, _) => Self::TooLong,
            _ => Self::Failed,
        }
    }
}

/// What differs per OS.
trait Os {
    /// Whether a store call can wait on an access or unlock prompt the person has not answered: an observation of the
    /// store; `admit` decides what follows from it. Default: it cannot.
    const PROMPTS: bool = false;

    /// This OS's store, opened for each operation so a failure never sticks. Its entries for `(service, provider)`
    /// carry the OS's own entry options.
    fn store() -> keyring_core::Result<Arc<CredentialStore>>;

    /// Delete every entry of exactly `service`, whatever its account. Default: the store's search for
    /// `{"service": <service>}`, which must find exactly that service's entries, then each entry it returns (`clear_in`).
    fn clear_namespace(service: &str) -> Result<(), Error> {
        clear_in(Self::store()?.as_ref(), service)
    }
}

/// Whether `provider` is a well-formed id: a namespace key, not necessarily a member of today's catalog.
pub fn valid(provider: &str) -> bool {
    (1..=32).contains(&provider.len())
        && provider.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A provider's key in the namespace `service`. Missing is `None`; inaccessible storage is `Unavailable`.
pub fn read(service: &str, provider: &str) -> Result<Option<String>, Error> {
    let provider = Provider::new(provider)?;
    let service = service.to_owned();
    admit(move || read_in(Native::store()?.as_ref(), &service, &provider))
}

/// Replace a key, or remove it for `None`. Removing a missing key succeeds.
/// Oversized credentials are refused before replacing the stored value; never truncated.
pub fn write(service: &str, provider: &str, secret: Option<&str>) -> Result<(), Error> {
    let provider = Provider::new(provider)?;
    let (service, secret) = (service.to_owned(), secret.map(str::to_owned));
    admit(move || write_in(Native::store()?.as_ref(), &service, &provider, secret.as_deref()))
}

/// Delete the exact service namespace `service`, including unknown or obsolete accounts.
/// An unrelated service is never removed; partial cleanup is an error.
pub fn clear_all(service: &str) -> Result<(), Error> {
    let service = service.to_owned();
    admit(move || Native::clear_namespace(&service))
}

/// A validated provider id, owned so it can cross to a blocking worker.
struct Provider(String);

impl Provider {
    fn new(provider: &str) -> Result<Self, Error> {
        if valid(provider) {
            Ok(Self(provider.to_owned()))
        } else {
            Err(Error::InvalidProvider)
        }
    }

    fn entry(&self, store: &CredentialStore, service: &str) -> keyring_core::Result<Entry> {
        store.build(service, &self.0, None)
    }
}

fn read_in(store: &CredentialStore, service: &str, provider: &Provider) -> Result<Option<String>, Error> {
    match provider.entry(store, service)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring_core::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn write_in(store: &CredentialStore, service: &str, provider: &Provider, secret: Option<&str>) -> Result<(), Error> {
    let entry = provider.entry(store, service)?;
    match secret {
        Some(secret) => entry.set_password(secret)?,
        None => match entry.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => (),
            Err(error) => return Err(error.into()),
        },
    }
    Ok(())
}

/// Delete each entry the store finds for `service`. Continues after a failure, but never reports partial cleanup as
/// success; an entry already gone is not a failure.
fn clear_in(store: &CredentialStore, service: &str) -> Result<(), Error> {
    let mut failure = None;
    for entry in store.search(&HashMap::from([("service", service)]))? {
        if let Err(error) = entry.delete_credential() {
            if !matches!(error, keyring_core::Error::NoEntry) {
                failure = Some(error);
            }
        }
    }
    failure.map_or(Ok(()), |error| Err(error.into()))
}

/// Admit one store operation. A store that can prompt gets bounded admission: callers wait at most `WAIT`, and at
/// most one operation is ever pending (see `bounded`). Any other store's operations are serialized within the process.
fn admit<T: Send + 'static>(operation: impl FnOnce() -> Result<T, Error> + Send + 'static) -> Result<T, Error> {
    static PENDING: AtomicBool = AtomicBool::new(false);
    static SERIAL: Mutex<()> = Mutex::new(());
    if Native::PROMPTS {
        bounded(&PENDING, WAIT, operation)
    } else {
        let _guard = SERIAL.lock().map_err(|_| Error::Failed)?;
        operation()
    }
}

// A synchronous keyring backend cannot cancel an outstanding prompt. Do not detach a new worker for every retry:
// retain one admission slot until the call really ends. A timed-out write has an unknown outcome; refusing
// overlapping calls prevents a late completion from overwriting a subsequent write or running after a successful clear.
fn bounded<T: Send + 'static>(
    busy: &'static AtomicBool,
    timeout: Duration,
    operation: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).map_err(|_| Error::Unavailable)?;
    struct Admission(&'static AtomicBool);
    impl Drop for Admission {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let admission = Admission(busy);
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("credentials".into())
        .spawn(move || {
            let result = operation();
            drop(admission);
            let _ = sender.send(result);
        })
        .map_err(|_| Error::Unavailable)?;
    receiver.recv_timeout(timeout).map_err(|_| Error::Unavailable)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyring_core::api::CredentialStoreApi;

    /// The service the memory store's fixtures use.
    const SERVICE: &str = "ai.sidevoice.test.providers";

    pub(super) fn provider(id: &str) -> Provider {
        Provider::new(id).unwrap()
    }

    /// Fixtures share a deterministic test lock, independent of the native admission state.
    pub(super) fn serial() -> std::sync::MutexGuard<'static, ()> {
        static ACCESS: Mutex<()> = Mutex::new(());
        ACCESS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Same assertions for the memory store and each native store, through the common requests.
    /// `clear` is the namespace clear under test: `clear_in` over `store`, or the OS's `Os::clear_namespace`.
    pub(super) fn contract(store: &CredentialStore, service: &str, clear: &dyn Fn(&str) -> Result<(), Error>) {
        let _serial = serial();
        let future = provider("future-provider-42");
        clear(service).unwrap();
        assert_eq!(read_in(store, service, &future).unwrap(), None);
        write_in(store, service, &future, None).unwrap();
        write_in(store, service, &future, Some("test-only-α🔐")).unwrap();
        assert_eq!(read_in(store, service, &future).unwrap().as_deref(), Some("test-only-α🔐"));
        write_in(store, service, &future, Some("replacement")).unwrap();
        assert_eq!(read_in(store, service, &future).unwrap().as_deref(), Some("replacement"));
        write_in(store, service, &future, None).unwrap();
        write_in(store, service, &future, None).unwrap();
        assert_eq!(read_in(store, service, &future).unwrap(), None);

        // Cleanup must include a future account that the current validator does not accept.
        let unknown = store.build(service, "future_provider", None).unwrap();
        unknown.set_password("test-only").unwrap();
        write_in(store, service, &provider("openai"), Some("test-only")).unwrap();
        let other_services = [format!("{service}.unrelated"), format!("unrelated.{service}")];
        let others: Vec<_> = other_services
            .iter()
            .map(|other| {
                let entry = store.build(other, "openai", None).unwrap();
                entry.set_password("unrelated-test-only").unwrap();
                entry
            })
            .collect();
        clear(service).unwrap();
        assert!(matches!(unknown.get_password(), Err(keyring_core::Error::NoEntry)));
        assert_eq!(read_in(store, service, &provider("openai")).unwrap(), None);
        for other in others {
            let retained = other.get_password();
            other.delete_credential().unwrap();
            assert_eq!(retained.unwrap(), "unrelated-test-only");
        }
        clear(service).unwrap();
    }

    /// A unique test service: native tests never read or clear real provider credentials, even on a developer's
    /// machine.
    pub(super) fn test_service() -> String {
        format!(
            "ai.sidevoice.test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        )
    }

    /// keyring's in-memory sample store, whose service search is an unanchored regex, made exact as `Os::store`
    /// requires of a native store.
    struct Memory(Arc<CredentialStore>);

    impl CredentialStoreApi for Memory {
        fn vendor(&self) -> String {
            self.0.vendor()
        }
        fn id(&self) -> String {
            self.0.id()
        }
        fn build(
            &self,
            service: &str,
            user: &str,
            modifiers: Option<&HashMap<&str, &str>>,
        ) -> keyring_core::Result<Entry> {
            self.0.build(service, user, modifiers)
        }
        fn search(&self, spec: &HashMap<&str, &str>) -> keyring_core::Result<Vec<Entry>> {
            let service = spec["service"];
            Ok(self
                .0
                .search(&HashMap::new())?
                .into_iter()
                .filter(|entry| entry.get_specifiers().is_some_and(|(found, _)| found == service))
                .collect())
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    fn memory() -> Memory {
        Memory(keyring_core::sample::Store::new().unwrap())
    }

    #[test]
    fn memory_contract() {
        let store = memory();
        contract(&store, SERVICE, &|service| clear_in(&store, service));
    }

    #[test]
    fn malformed_ids_cannot_address_credentials() {
        for id in ["", "OpenAI", "with.dot", "with/slash", "with space", "é", &"a".repeat(33)] {
            assert!(!valid(id));
            assert_eq!(read(SERVICE, id), Err(Error::InvalidProvider));
            assert_eq!(write(SERVICE, id, Some("test-only")), Err(Error::InvalidProvider));
            assert_eq!(write(SERVICE, id, None), Err(Error::InvalidProvider));
        }
        assert!(valid(&"a".repeat(32)));
        let store = memory();
        write_in(&store, SERVICE, &provider(&"a".repeat(32)), Some("test-only")).unwrap();
    }

    #[test]
    fn backend_errors_discard_all_secret_bearing_details() {
        let secret = "must-never-reach-an-error";
        let failures = [
            keyring_core::Error::BadEncoding(secret.as_bytes().to_vec()),
            keyring_core::Error::BadDataFormat(secret.as_bytes().to_vec(), std::io::Error::other(secret).into()),
            keyring_core::Error::Invalid(secret.into(), secret.into()),
            keyring_core::Error::PlatformFailure(std::io::Error::other(secret).into()),
            keyring_core::Error::NoStorageAccess(std::io::Error::other(secret).into()),
        ];
        for failure in failures {
            assert!(!format!("{:?}", Error::from(failure)).contains(secret));
        }
        assert_eq!(
            Error::from(keyring_core::Error::NoStorageAccess(std::io::Error::other(secret).into())),
            Error::Unavailable
        );
        assert_eq!(Error::from(keyring_core::Error::TooLong("password".into(), 2560)), Error::TooLong);
    }

    #[test]
    fn invalid_stored_text_is_an_error_not_a_missing_key() {
        let store = memory();
        store.build(SERVICE, "openai", None).unwrap().set_secret(&[0xff]).unwrap();
        assert_eq!(read_in(&store, SERVICE, &provider("openai")), Err(Error::Failed));
        clear_in(&store, SERVICE).unwrap();
    }

    #[test]
    fn ambiguous_accounts_are_removed_by_namespace_clear() {
        let store = memory();
        let modifiers = HashMap::from([("force-create", "true")]);
        store.build(SERVICE, "openai", Some(&modifiers)).unwrap();
        store.build(SERVICE, "openai", Some(&modifiers)).unwrap();
        assert_eq!(read_in(&store, SERVICE, &provider("openai")), Err(Error::Failed));
        clear_in(&store, SERVICE).unwrap();
        assert_eq!(read_in(&store, SERVICE, &provider("openai")).unwrap(), None);
    }

    #[test]
    fn slow_backend_times_out_without_queuing_later_operations() {
        use std::time::Instant;
        static BUSY: AtomicBool = AtomicBool::new(false);
        let (release, blocked) = mpsc::channel();
        let (finished, completion) = mpsc::channel();
        let started = Instant::now();
        // A fake backend waiting for an unanswered prompt. No real store needed.
        assert_eq!(
            bounded(&BUSY, Duration::from_millis(20), move || {
                blocked.recv().unwrap();
                finished.send(()).unwrap();
                Ok(())
            }),
            Err(Error::Unavailable)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(BUSY.load(Ordering::Acquire));
        let started = Instant::now();
        assert_eq!(
            bounded(&BUSY, Duration::from_secs(5), || -> Result<(), Error> {
                panic!("a retry must not queue another backend operation")
            }),
            Err(Error::Unavailable)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        release.send(()).unwrap();
        completion.recv_timeout(Duration::from_secs(2)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while BUSY.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(bounded(&BUSY, Duration::from_secs(2), || Ok(42)), Ok(42));
    }
}
