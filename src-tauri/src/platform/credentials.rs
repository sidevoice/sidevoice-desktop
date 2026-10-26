//! Provider secrets in the OS credential store. No store error or secret is formatted or logged.
//! The service/account pair is the original macOS namespace, not a catalog-dependent list.

use keyring_core::{CredentialStore, Entry};
use std::{collections::HashMap, sync::Arc};

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

/// Provider keys in the native store. Backend admission and storage are sealed inside this concern.
#[allow(private_bounds)]
pub trait Credentials: Backend {
    /// A provider id is a namespace key, not necessarily a member of today's catalog.
    fn valid(provider: &str) -> bool
    where
        Self: Sized,
    {
        Provider::valid(provider)
    }

    /// Read a provider's key. Missing is `None`; inaccessible storage is `Unavailable`.
    fn read(&self, provider: &str) -> Result<Option<String>, Error> {
        self.execute(Request::Read(Provider::new(provider)?))
    }

    /// Replace a key, or remove it for `None`. Removing a missing key succeeds.
    /// Oversized credentials are refused before replacing the stored value; never truncated.
    fn write(&self, provider: &str, secret: Option<&str>) -> Result<(), Error> {
        self.execute(Request::Write(Provider::new(provider)?, secret.map(str::to_owned))).map(|_| ())
    }

    /// Delete the exact service namespace, including unknown or obsolete accounts.
    /// An unrelated service is never removed; partial cleanup is an error.
    fn clear(&self) -> Result<(), Error> {
        self.execute(Request::Clear).map(|_| ())
    }
}

/// Private implementation hook: callers can only perform the documented credential operations.
trait Backend {
    /// Execute a validated request using the native store's admission and persistence policy.
    fn execute(&self, request: Request) -> Result<Option<String>, Error>;
}

/// What differs per OS, implemented once by each `native::Store`: the store, entry options, the namespace
/// search, and admission. Validation, request handling and error categories are shared.
trait NativeStore {
    /// The native store, opened for each operation so a failure never sticks.
    fn store() -> keyring_core::Result<Arc<CredentialStore>>;

    /// One provider's entry. Default: the store's entry without options.
    fn entry(store: &CredentialStore, service: &str, provider: &str) -> keyring_core::Result<Entry> {
        store.build(service, provider, None)
    }

    /// The search specifier selecting exactly `service`'s entries, whatever their account.
    /// Default: keyring's exact `service` attribute.
    fn namespace(service: &str) -> (&'static str, String) {
        ("service", service.to_owned())
    }

    /// Delete every entry `namespace` finds.
    fn clear(store: &CredentialStore, service: &str) -> keyring_core::Result<()> {
        let (key, value) = Self::namespace(service);
        delete(store.search(&HashMap::from([(key, value.as_str())]))?)
    }

    /// Admit one operation. Default: operations are serialized within the process.
    fn admit(operation: Operation) -> Result<Option<String>, Error> {
        static ACCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ACCESS.lock().map_err(|_| Error::Failed)?;
        operation()
    }
}

/// One validated request bound to the native store, movable to a worker thread.
type Operation = Box<dyn FnOnce() -> Result<Option<String>, Error> + Send>;

/// The provider keys in this OS's credential store. A failure never installs a fallback or prevents a later retry.
pub struct ProviderCredentials;

impl Credentials for ProviderCredentials {}

impl Backend for ProviderCredentials {
    fn execute(&self, request: Request) -> Result<Option<String>, Error> {
        use native::Store as Native;
        Native::admit(Box::new(move || request.run(Native::store()?.as_ref(), SERVICE, Native::entry, Native::clear)))
    }
}

struct Provider(String);

impl Provider {
    fn valid(provider: &str) -> bool {
        (1..=32).contains(&provider.len())
            && provider.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }

    fn new(provider: &str) -> Result<Self, Error> {
        if Self::valid(provider) {
            Ok(Self(provider.to_owned()))
        } else {
            Err(Error::InvalidProvider)
        }
    }
}

/// Owned, validated input can cross a blocking worker boundary without borrowing caller data.
enum Request {
    Read(Provider),
    Write(Provider, Option<String>),
    Clear,
}

impl Request {
    /// Shared keyring behavior; entry construction and exact namespace deletion come from the backend.
    fn run(
        self,
        store: &CredentialStore,
        service: &str,
        entry: fn(&CredentialStore, &str, &str) -> keyring_core::Result<Entry>,
        clear: fn(&CredentialStore, &str) -> keyring_core::Result<()>,
    ) -> Result<Option<String>, Error> {
        match self {
            Self::Read(provider) => match entry(store, service, &provider.0)?.get_password() {
                Ok(secret) => Ok(Some(secret)),
                Err(keyring_core::Error::NoEntry) => Ok(None),
                Err(error) => Err(error.into()),
            },
            Self::Write(provider, secret) => {
                let entry = entry(store, service, &provider.0)?;
                match secret {
                    Some(secret) => entry.set_password(&secret)?,
                    None => match entry.delete_credential() {
                        Ok(()) | Err(keyring_core::Error::NoEntry) => (),
                        Err(error) => return Err(error.into()),
                    },
                }
                Ok(None)
            }
            Self::Clear => {
                clear(store, service)?;
                Ok(None)
            }
        }
    }
}

/// Delete each entry. Continues after a failure, but never reports partial cleanup as success;
/// an entry already gone is not a failure.
fn delete(entries: Vec<Entry>) -> keyring_core::Result<()> {
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

    pub(super) struct TestCredentials {
        pub(super) store: Arc<CredentialStore>,
        pub(super) service: String,
        pub(super) clear: fn(&CredentialStore, &str) -> keyring_core::Result<()>,
    }

    impl Credentials for TestCredentials {}

    impl Backend for TestCredentials {
        fn execute(&self, request: Request) -> Result<Option<String>, Error> {
            // Fixtures share a deterministic test lock, independent of native admission state.
            static ACCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let _guard = ACCESS.lock().map_err(|_| Error::Failed)?;
            request.run(self.store.as_ref(), &self.service, native::Store::entry, self.clear)
        }
    }

    fn memory_clear(store: &CredentialStore, service: &str) -> keyring_core::Result<()> {
        // The sample store's service matcher is a regex, unlike native macOS/Linux stores.
        let entries = store.search(&HashMap::new())?;
        delete(
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
    pub(super) fn contract(store: &TestCredentials) {
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
            assert!(!ProviderCredentials::valid(id));
            assert_eq!(ProviderCredentials.read(id), Err(Error::InvalidProvider));
            assert_eq!(ProviderCredentials.write(id, Some("test-only")), Err(Error::InvalidProvider));
            assert_eq!(store.read(id), Err(Error::InvalidProvider));
            assert_eq!(store.write(id, Some("test-only")), Err(Error::InvalidProvider));
            assert_eq!(store.write(id, None), Err(Error::InvalidProvider));
        }
        assert!(ProviderCredentials::valid(&"a".repeat(32)));
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

    pub(super) fn native_test_store() -> TestCredentials {
        // Never read or clear real provider credentials, even on a developer's machine.
        TestCredentials {
            store: native::Store::store().unwrap(),
            service: format!(
                "dev.sidevoice.desktop.test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
            ),
            clear: native::Store::clear,
        }
    }
}
