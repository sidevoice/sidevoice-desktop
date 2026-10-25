//! Provider secrets in the OS credential store. No store error or secret is formatted or logged.
//! The service/account pair is the original macOS namespace, not a catalog-dependent list.

use keyring_core::{CredentialStore, Entry};
#[cfg(not(target_os = "linux"))]
use std::sync::Mutex;

#[cfg(target_os = "macos")]
#[path = "credentials/macos.rs"]
mod native;
#[cfg(target_os = "windows")]
#[path = "credentials/windows.rs"]
mod native;
#[cfg(target_os = "linux")]
#[path = "credentials/linux.rs"]
mod native;

const SERVICE: &str = "dev.sidevoice.desktop.providers";
// CredMan does not reliably sequence concurrent operations on the same entry. Linux uses
// bounded, nonblocking admission instead: an unlock prompt must never hold this lock.
#[cfg(not(target_os = "linux"))]
static ACCESS: Mutex<()> = Mutex::new(());

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

/// A provider id is a namespace key, not necessarily a member of today's catalog.
pub fn valid(provider: &str) -> bool {
    (1..=32).contains(&provider.len())
        && provider.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub trait Credentials {
    /// Read a provider's key. Missing is `None`; inaccessible storage is `Unavailable`.
    fn read(&self, provider: &str) -> Result<Option<String>, Error> {
        if !valid(provider) {
            return Err(Error::InvalidProvider);
        }
        let provider = provider.to_owned();
        execute(move || read_from_store(native::store().map_err(Error::from)?.as_ref(), SERVICE, &provider))
    }
    /// Replace a key, or remove it for `None`. Removing a missing key succeeds.
    /// Oversized credentials are refused before replacing the stored value; never truncated.
    fn write(&self, provider: &str, secret: Option<&str>) -> Result<(), Error> {
        if !valid(provider) {
            return Err(Error::InvalidProvider);
        }
        let provider = provider.to_owned();
        let secret = secret.map(str::to_owned);
        execute(move || {
            write_to_store(native::store().map_err(Error::from)?.as_ref(), SERVICE, &provider, secret.as_deref())
        })
    }
    /// Delete the exact service namespace, including accounts absent from the current catalog
    /// or no longer accepted by `valid`. An unrelated service is never removed.
    // The production reset caller is still macOS-only until the portable-reset work lands.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn clear(&self) -> Result<(), Error> {
        execute(|| native::clear(native::store().map_err(Error::from)?.as_ref(), SERVICE).map_err(Error::from))
    }
}

/// Native keyring stores, selected inside this concern. Each operation opens the store anew,
/// so failure never installs a fallback or prevents retry after the store becomes accessible.
pub struct ProviderCredentials;
impl Credentials for ProviderCredentials {}

/// The Linux backend may wait for a desktop prompt. Bound the caller's wait without
/// holding ACCESS or spawning an unlimited number of workers. Other native stores keep
/// their existing serialized behavior.
fn execute<T: Send + 'static>(operation: impl FnOnce() -> Result<T, Error> + Send + 'static) -> Result<T, Error> {
    #[cfg(target_os = "linux")]
    {
        native::execute(operation)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _guard = ACCESS.lock().map_err(|_| Error::Failed)?;
        operation()
    }
}

fn entry(store: &CredentialStore, service: &str, provider: &str) -> Result<Entry, Error> {
    if !valid(provider) {
        return Err(Error::InvalidProvider);
    }
    #[cfg(target_os = "windows")]
    if store.as_any().is::<windows_native_keyring_store::Store>() {
        return native::entry(store, service, provider).map_err(Error::from);
    }
    store.build(service, provider, None).map_err(Error::from)
}

fn read_from_store(store: &CredentialStore, service: &str, provider: &str) -> Result<Option<String>, Error> {
    match entry(store, service, provider)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring_core::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn write_to_store(store: &CredentialStore, service: &str, provider: &str, secret: Option<&str>) -> Result<(), Error> {
    let entry = entry(store, service, provider)?;
    match secret {
        Some(secret) => entry.set_password(secret).map_err(Error::from),
        None => delete(&entry),
    }
}

fn delete(entry: &Entry) -> Result<(), Error> {
    match entry.delete_credential() {
        Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Continue deleting other entries after a failure, but never report partial cleanup as success.
#[cfg(any(not(target_os = "macos"), test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn delete_entries(entries: Vec<Entry>) -> keyring_core::Result<()> {
    let mut failure = None;
    for entry in entries {
        if let Err(error) = entry.delete_credential() {
            if !matches!(error, keyring_core::Error::NoEntry) {
                failure = Some(error);
            }
        }
    }
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Arc};

    struct TestCredentials {
        store: Arc<CredentialStore>,
        service: String,
        clear: fn(&CredentialStore, &str) -> keyring_core::Result<()>,
    }

    impl Credentials for TestCredentials {
        fn read(&self, provider: &str) -> Result<Option<String>, Error> {
            #[cfg(not(target_os = "linux"))]
            let _guard = ACCESS.lock().map_err(|_| Error::Failed)?;
            read_from_store(self.store.as_ref(), &self.service, provider)
        }
        fn write(&self, provider: &str, secret: Option<&str>) -> Result<(), Error> {
            #[cfg(not(target_os = "linux"))]
            let _guard = ACCESS.lock().map_err(|_| Error::Failed)?;
            write_to_store(self.store.as_ref(), &self.service, provider, secret)
        }
        fn clear(&self) -> Result<(), Error> {
            #[cfg(not(target_os = "linux"))]
            let _guard = ACCESS.lock().map_err(|_| Error::Failed)?;
            (self.clear)(self.store.as_ref(), &self.service).map_err(Error::from)
        }
    }

    fn memory_clear(store: &CredentialStore, service: &str) -> keyring_core::Result<()> {
        // The sample store's service matcher is a regex, unlike native macOS/Linux stores.
        let entries = store.search(&HashMap::new())?;
        delete_entries(
            entries
                .into_iter()
                .filter(|entry| entry.get_specifiers().is_some_and(|(found, _)| found == service))
                .collect(),
        )
    }

    fn memory() -> TestCredentials {
        TestCredentials {
            store: keyring_core::sample::Store::new().unwrap(),
            service: SERVICE.into(),
            clear: memory_clear,
        }
    }

    /// Same assertions through the interface, for the memory store and each native store.
    fn contract(store: &TestCredentials) {
        let credentials: &dyn Credentials = store;
        credentials.clear().unwrap();
        assert_eq!(credentials.read("future-provider-42").unwrap(), None);
        credentials.write("future-provider-42", None).unwrap();
        credentials.write("future-provider-42", Some("test-only-α🔐")).unwrap();
        assert_eq!(credentials.read("future-provider-42").unwrap().as_deref(), Some("test-only-α🔐"));
        credentials.write("future-provider-42", Some("replacement")).unwrap();
        assert_eq!(credentials.read("future-provider-42").unwrap().as_deref(), Some("replacement"));
        credentials.write("future-provider-42", None).unwrap();
        credentials.write("future-provider-42", None).unwrap();
        assert_eq!(credentials.read("future-provider-42").unwrap(), None);

        // Cleanup must include a future account that the current validator does not accept.
        let unknown = store.store.build(&store.service, "future_provider", None).unwrap();
        unknown.set_password("test-only").unwrap();
        credentials.write("openai", Some("test-only")).unwrap();
        let other_services = [format!("{}.unrelated", store.service), format!("unrelated.{}", store.service)];
        let others: Vec<_> = other_services
            .iter()
            .map(|service| {
                let entry = store.store.build(service, "openai", None).unwrap();
                entry.set_password("unrelated-test-only").unwrap();
                entry
            })
            .collect();
        credentials.clear().unwrap();
        assert!(matches!(unknown.get_password(), Err(keyring_core::Error::NoEntry)));
        assert_eq!(credentials.read("openai").unwrap(), None);
        for other in others {
            let retained = other.get_password();
            other.delete_credential().unwrap();
            assert_eq!(retained.unwrap(), "unrelated-test-only");
        }
        credentials.clear().unwrap();
    }

    #[test]
    fn memory_contract() {
        let _native: &dyn Credentials = &ProviderCredentials;
        contract(&memory());
    }

    #[test]
    fn malformed_ids_cannot_address_credentials() {
        let store = memory();
        for id in ["", "OpenAI", "with.dot", "with/slash", "with space", "é", &"a".repeat(33)] {
            assert_eq!(store.read(id), Err(Error::InvalidProvider));
            assert_eq!(store.write(id, Some("test-only")), Err(Error::InvalidProvider));
            assert_eq!(store.write(id, None), Err(Error::InvalidProvider));
        }
        store.write(&"a".repeat(32), Some("test-only")).unwrap();
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
        store.store.build(&store.service, "openai", None).unwrap().set_secret(&[0xff]).unwrap();
        assert_eq!(store.read("openai"), Err(Error::Failed));
        store.clear().unwrap();
    }

    #[test]
    fn ambiguous_accounts_are_removed_by_namespace_clear() {
        let store = memory();
        let modifiers = HashMap::from([("force-create", "true")]);
        store.store.build(&store.service, "openai", Some(&modifiers)).unwrap();
        store.store.build(&store.service, "openai", Some(&modifiers)).unwrap();
        assert_eq!(store.read("openai"), Err(Error::Failed));
        store.clear().unwrap();
        assert_eq!(store.read("openai").unwrap(), None);
    }

    fn native_test_store() -> TestCredentials {
        // Never read or clear real provider credentials, even on a developer's machine.
        TestCredentials {
            store: native::store().unwrap(),
            service: format!(
                "dev.sidevoice.desktop.test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
            ),
            clear: native::clear,
        }
    }

    #[test]
    #[cfg_attr(target_os = "linux", ignore = "requires an unlocked desktop Secret Service; unverified on headless CI")]
    fn native_contract() {
        let store = native_test_store();
        contract(&store);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn legacy_macos_passwords_remain_readable_in_both_directions() {
        use security_framework::passwords::{get_generic_password, set_generic_password};
        let store = native_test_store();
        set_generic_password(&store.service, "openai", b"legacy-test-only").unwrap();
        assert_eq!(store.read("openai").unwrap().as_deref(), Some("legacy-test-only"));
        store.write("openai", Some("updated-test-only")).unwrap();
        assert_eq!(get_generic_password(&store.service, "openai").unwrap(), b"updated-test-only");
        set_generic_password(&store.service, "legacy_unknown", b"legacy-test-only").unwrap();
        store.clear().unwrap();
        assert_eq!(get_generic_password(&store.service, "legacy_unknown").unwrap_err().code(), -25300);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_size_limit_rejects_without_replacing() {
        let store = native_test_store();
        for (fits, too_big) in [("a".repeat(1280), "a".repeat(1281)), ("🔐".repeat(640), "🔐".repeat(641))] {
            store.write("openai", Some(&fits)).unwrap();
            assert_eq!(store.write("openai", Some(&too_big)), Err(Error::TooLong));
            assert_eq!(store.read("openai").unwrap().as_deref(), Some(fits.as_str()));
        }
        store.clear().unwrap();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn slow_backend_times_out_without_queuing_later_operations() {
        use std::{
            sync::{
                atomic::{AtomicBool, Ordering},
                mpsc,
            },
            time::{Duration, Instant},
        };
        static BUSY: AtomicBool = AtomicBool::new(false);
        let (release, blocked) = mpsc::channel();
        let (finished, completion) = mpsc::channel();
        let started = Instant::now();
        // A fake backend waiting for an unanswered prompt. No real Secret Service needed.
        assert_eq!(
            native::bounded(&BUSY, Duration::from_millis(20), move || {
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
            native::bounded(&BUSY, Duration::from_secs(5), || -> Result<(), Error> {
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
        assert_eq!(native::bounded(&BUSY, Duration::from_secs(2), || Ok(42)), Ok(42));
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_credentials_persist_only_on_this_computer() {
        let store = native_test_store();
        // Read metadata back from CredMan, not just from the entry's creation modifiers.
        store.write("openai", Some("test-only")).unwrap();
        let raw = store.store.build(&store.service, "openai", None).unwrap();
        assert_eq!(raw.get_attributes().unwrap()["persistence"], "Local");
        // Updating an entry written by the previous implementation also removes roaming.
        raw.set_password("old-enterprise-test-only").unwrap();
        assert_eq!(raw.get_attributes().unwrap()["persistence"], "Enterprise");
        store.write("openai", Some("replacement")).unwrap();
        assert_eq!(raw.get_attributes().unwrap()["persistence"], "Local");
        store.clear().unwrap();
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_searches_nondefault_keychains_for_read_update_delete_and_clear() {
        use security_framework::os::macos::keychain::SecKeychain;
        use std::process::Command;
        fn security(args: &[&str]) -> std::process::Output {
            let output = Command::new("/usr/bin/security").args(args).output().unwrap();
            assert!(output.status.success(), "security test setup/cleanup failed");
            output
        }
        const CHILD: &str = "SIDEVOICE_CREDENTIAL_SEARCH_LIST_TEST";
        const KEYCHAIN_PATH: &str = "SIDEVOICE_CREDENTIAL_KEYCHAIN_PATH";
        if let Ok(service) = std::env::var(CHILD) {
            let store = TestCredentials { store: native::store().unwrap(), service, clear: native::clear };
            let secondary = SecKeychain::open(std::env::var(KEYCHAIN_PATH).unwrap()).unwrap();
            // Establish that the fixture is reachable by the old app's lookup API too.
            assert_eq!(
                security_framework::passwords::get_generic_password(&store.service, "openai").unwrap(),
                b"legacy-secondary"
            );
            assert_eq!(store.read("openai").unwrap().as_deref(), Some("legacy-secondary"));
            store.write("openai", Some("updated-secondary")).unwrap();
            use security_framework::os::macos::passwords::find_generic_password;
            assert_eq!(
                find_generic_password(Some(std::slice::from_ref(&secondary)), &store.service, "openai")
                    .unwrap()
                    .0
                    .as_ref(),
                b"updated-secondary"
            );
            store.write("openai", None).unwrap();
            assert_eq!(
                find_generic_password(Some(std::slice::from_ref(&secondary)), &store.service, "openai")
                    .unwrap_err()
                    .code(),
                -25300
            );
            secondary.set_generic_password(&store.service, "unknown_old", b"test-only").unwrap();
            store.clear().unwrap();
            assert_eq!(
                find_generic_password(Some(&[secondary]), &store.service, "unknown_old").unwrap_err().code(),
                -25300
            );
            return;
        }
        // Preserve the runner/developer's complete search list, including on assertion failure.
        struct Keychain {
            path: String,
            previous: Vec<String>,
        }
        impl Drop for Keychain {
            fn drop(&mut self) {
                let _ = Command::new("/usr/bin/security")
                    .args(["list-keychains", "-d", "user", "-s"])
                    .args(&self.previous)
                    .output();
                let _ = Command::new("/usr/bin/security").args(["delete-keychain", &self.path]).output();
            }
        }
        let store = native_test_store();
        let output = security(&["list-keychains", "-d", "user"]);
        let previous: Vec<String> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| line.trim().trim_matches('"').to_owned())
            .collect();
        let keychain = Keychain {
            path: std::env::temp_dir().join(format!("{}.keychain-db", store.service)).to_str().unwrap().into(),
            previous,
        };
        security(&["create-keychain", "-p", "test-only", &keychain.path]);
        security(&["unlock-keychain", "-p", "test-only", &keychain.path]);
        let mut search = vec!["list-keychains", "-d", "user", "-s"];
        search.extend(keychain.previous.iter().map(String::as_str));
        search.push(&keychain.path);
        security(&search);
        let secondary = SecKeychain::open(&keychain.path).unwrap();
        secondary.set_generic_password(&store.service, "openai", b"legacy-secondary").unwrap();
        // Security.framework caches the search list per process. Other tests may already
        // have used it before the security CLI added this fixture; start a fresh process.
        let status = Command::new(std::env::current_exe().unwrap())
            .arg("macos_searches_nondefault_keychains_for_read_update_delete_and_clear")
            .env(CHILD, &store.service)
            .env(KEYCHAIN_PATH, &keychain.path)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn missing_secret_service_is_explicitly_unavailable() {
        const CHILD: &str = "SIDEVOICE_CREDENTIAL_UNAVAILABLE_TEST";
        if std::env::var_os(CHILD).is_some() {
            assert_eq!(ProviderCredentials.read("openai"), Err(Error::Unavailable));
            assert_eq!(ProviderCredentials.write("openai", Some("test-only")), Err(Error::Unavailable));
            assert_eq!(ProviderCredentials.clear(), Err(Error::Unavailable));
            return;
        }
        // Isolate the bus environment from concurrent tests and any real user's keyring.
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("missing_secret_service_is_explicitly_unavailable")
            .env(CHILD, "1")
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent-sidevoice-test-bus")
            .status()
            .unwrap();
        assert!(status.success());
    }
}
