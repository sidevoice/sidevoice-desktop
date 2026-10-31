//! Provider secrets in the OS credential store (docs/CREDENTIALS.md). No store error or secret is formatted or logged.
//! The service/account pair is the original macOS namespace, not a catalog-dependent list.
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
mod imp;
use imp::Native;

const SERVICE: &str = "ai.sidevoice.desktop.providers";

/// The namespace the public functions use: `SERVICE`. Test builds give them a per-process namespace of their own, so
/// fixtures of the public functions on a native store never read or clear a person's keys.
fn namespace() -> &'static str {
    #[cfg(not(test))]
    return SERVICE;
    #[cfg(test)]
    {
        static TEST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        TEST.get_or_init(tests::test_service)
    }
}

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

/// A provider's key. Missing is `None`; inaccessible storage is `Unavailable`.
pub fn read(provider: &str) -> Result<Option<String>, Error> {
    let provider = Provider::new(provider)?;
    admit(move || read_in(Native::store()?.as_ref(), namespace(), &provider))
}

/// Replace a key, or remove it for `None`. Removing a missing key succeeds.
/// Oversized credentials are refused before replacing the stored value; never truncated.
pub fn write(provider: &str, secret: Option<&str>) -> Result<(), Error> {
    let provider = Provider::new(provider)?;
    let secret = secret.map(str::to_owned);
    admit(move || write_in(Native::store()?.as_ref(), namespace(), &provider, secret.as_deref()))
}

/// Delete the exact service namespace, including unknown or obsolete accounts.
/// An unrelated service is never removed; partial cleanup is an error.
pub fn clear_all() -> Result<(), Error> {
    admit(|| Native::clear_namespace(namespace()))
}

/// The providers with a key in the namespace the app kept under the bundle identifier `identifier` (its
/// `<identifier>.providers` service): the accounts there that are valid provider ids. `Err` when it cannot be listed.
pub fn providers_in(identifier: &str) -> Result<Vec<String>, Error> {
    let service = service_of(identifier);
    admit(move || providers_of(Native::store()?.as_ref(), &service))
}

/// Copy `provider`'s key from the namespace of the bundle identifier `identifier` to the one the public functions use,
/// unless that one holds a key for it: `Ok(true)` copied and read back, `Ok(false)` nothing to copy. Nothing is
/// overwritten or deleted. One key per call, each with its own admission and deadline. `settle` runs on the store's
/// worker once both namespaces were read and before anything is written, and nothing is written unless it returns
/// true: it runs even for a copy that completes after this call stopped waiting (`Unavailable` at the deadline), so
/// the caller can record that copy there.
pub fn copy_key_from(
    identifier: &str,
    provider: &str,
    settle: impl FnOnce() -> bool + Send + 'static,
) -> Result<bool, Error> {
    let (from, provider) = (service_of(identifier), Provider::new(provider)?);
    admit(move || copy_key(Native::store()?.as_ref(), &from, namespace(), &provider, settle))
}

/// The service of the namespace that a build with the bundle identifier `identifier` kept its provider keys in.
fn service_of(identifier: &str) -> String {
    format!("{identifier}.providers")
}

fn providers_of(store: &CredentialStore, service: &str) -> Result<Vec<String>, Error> {
    let entries = store.search(&HashMap::from([("service", service)]))?;
    let accounts = entries.iter().filter_map(Entry::get_specifiers);
    Ok(accounts.filter(|(found, account)| found == service && valid(account)).map(|(_, account)| account).collect())
}

/// [`copy_key_from`] the service `from` to the service `to` in `store`.
fn copy_key(
    store: &CredentialStore,
    from: &str,
    to: &str,
    provider: &Provider,
    settle: impl FnOnce() -> bool,
) -> Result<bool, Error> {
    let secret = match read_in(store, to, provider)? {
        Some(_) => None,
        None => read_in(store, from, provider)?,
    };
    if !settle() {
        return Err(Error::Failed);
    }
    let Some(secret) = secret else { return Ok(false) };
    write_in(store, to, provider, Some(&secret))?;
    match read_in(store, to, provider)? {
        Some(read) if read == secret => Ok(true),
        _ => Err(Error::Failed),
    }
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
            "ai.sidevoice.desktop.test-{}-{}",
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
            assert_eq!(read(id), Err(Error::InvalidProvider));
            assert_eq!(write(id, Some("test-only")), Err(Error::InvalidProvider));
            assert_eq!(write(id, None), Err(Error::InvalidProvider));
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

    /// [`copy_key_from`] through the real store (macOS and Windows run it; Linux has no store on headless CI): a key
    /// under the namespace of another identifier, unique to this test, is copied to the namespace the public functions
    /// use in test builds, and stays in the old one.
    pub(super) fn copy_native() {
        let _serial = serial();
        let identifier = test_service();
        let old = service_of(&identifier);
        let store = Native::store().unwrap();
        write_in(store.as_ref(), &old, &provider("openai"), Some("test-only")).unwrap();
        assert_eq!(providers_in(&identifier), Ok(vec!["openai".to_owned()]));
        assert_eq!(copy_key_from(&identifier, "openai", || true), Ok(true));
        assert_eq!(read("openai").unwrap().as_deref(), Some("test-only"));
        assert_eq!(read_in(store.as_ref(), &old, &provider("openai")).unwrap().as_deref(), Some("test-only"));
        assert_eq!(copy_key_from(&identifier, "openai", || true), Ok(false));
        clear_all().unwrap();
        clear_in(store.as_ref(), &old).unwrap();
    }

    /// The namespace of the builds before `ai.sidevoice.desktop` (`legacy_identifier`).
    const OLD: &str = "dev.sidevoice.desktop.providers";

    fn copy(store: &CredentialStore, id: &str) -> Result<bool, Error> {
        copy_key(store, OLD, SERVICE, &provider(id), || true)
    }

    fn key(store: &CredentialStore, service: &str, id: &str) -> Option<String> {
        read_in(store, service, &provider(id)).unwrap()
    }

    #[test]
    fn a_namespace_is_its_identifier_plus_providers() {
        assert_eq!(service_of("ai.sidevoice.desktop"), SERVICE);
        assert_eq!(service_of("dev.sidevoice.desktop"), OLD);
    }

    #[test]
    fn keys_are_copied_to_the_new_namespace_and_stay_in_the_old_one() {
        let _serial = serial();
        let store = memory();
        write_in(&store, OLD, &provider("openai"), Some("old-openai")).unwrap();
        write_in(&store, OLD, &provider("future-provider-42"), Some("old-future")).unwrap();
        // Not the app's: an account its validator refuses, and another service with a valid one.
        let foreign = store.build(OLD, "future_provider", None).unwrap();
        foreign.set_password("keep").unwrap();
        write_in(&store, "unrelated.service", &provider("openai"), Some("keep")).unwrap();

        let mut providers = providers_of(&store, OLD).unwrap();
        providers.sort();
        assert_eq!(providers, ["future-provider-42", "openai"]);
        assert_eq!((copy(&store, "openai"), copy(&store, "future-provider-42")), (Ok(true), Ok(true)));
        assert_eq!(key(&store, SERVICE, "openai").as_deref(), Some("old-openai"));
        assert_eq!(key(&store, SERVICE, "future-provider-42").as_deref(), Some("old-future"));
        assert_eq!(key(&store, OLD, "openai").as_deref(), Some("old-openai"));
        assert_eq!(key(&store, OLD, "future-provider-42").as_deref(), Some("old-future"));
        assert_eq!(foreign.get_password().unwrap(), "keep");
        assert_eq!(key(&store, "unrelated.service", "openai").as_deref(), Some("keep"));
    }

    #[test]
    fn an_old_key_and_a_different_new_one_are_both_kept() {
        let _serial = serial();
        let store = memory();
        write_in(&store, OLD, &provider("openai"), Some("old")).unwrap();
        write_in(&store, SERVICE, &provider("openai"), Some("new")).unwrap();
        assert_eq!(copy(&store, "openai"), Ok(false));
        assert_eq!(key(&store, SERVICE, "openai").as_deref(), Some("new"));
        assert_eq!(key(&store, OLD, "openai").as_deref(), Some("old"));
    }

    #[test]
    fn nothing_is_written_unless_it_is_settled_and_an_unreadable_key_is_not_settled() {
        let _serial = serial();
        let store = memory();
        write_in(&store, OLD, &provider("openai"), Some("old")).unwrap();
        assert_eq!(copy_key(&store, OLD, SERVICE, &provider("openai"), || false), Err(Error::Failed));
        assert_eq!(key(&store, SERVICE, "openai"), None);
        // Stored text that is not text: reading it fails before anything is settled.
        store.build(OLD, "other", None).unwrap().set_secret(&[0xff]).unwrap();
        let settled = || unreachable!("not settled");
        assert_eq!(copy_key(&store, OLD, SERVICE, &provider("other"), settled), Err(Error::Failed));
        assert_eq!(store.build(OLD, "other", None).unwrap().get_secret().unwrap(), vec![0xff]);
    }

    /// A copy whose admission stopped waiting for it (a Keychain prompt answered late) still completes, and is settled
    /// where it does: the caller records it. The next key meanwhile is refused at once, without running.
    #[test]
    fn a_copy_that_completes_after_its_deadline_is_settled_all_the_same() {
        static BUSY: AtomicBool = AtomicBool::new(false);
        let _serial = serial();
        let store = memory();
        write_in(&store, OLD, &provider("openai"), Some("old")).unwrap();
        let (worker, (settled, recorded)) = (Memory(store.0.clone()), mpsc::channel());
        let late = bounded(&BUSY, Duration::from_millis(20), move || {
            std::thread::sleep(Duration::from_millis(200));
            copy_key(&worker, OLD, SERVICE, &provider("openai"), move || settled.send(()).is_ok())
        });
        assert_eq!(late, Err(Error::Unavailable));
        let next = bounded(&BUSY, Duration::from_millis(20), || -> Result<bool, Error> { unreachable!("busy") });
        assert_eq!(next, Err(Error::Unavailable));
        recorded.recv_timeout(Duration::from_secs(10)).unwrap();
        while BUSY.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(key(&store, SERVICE, "openai").as_deref(), Some("old"));
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
