//! The provider keys in the OS credential store (platform/credentials, docs/CREDENTIALS.md), in the namespace the app
//! passes in: the one it has always used, exactly, so the keys it already holds are the keys it finds.

use super::{KeyError, ProviderKeys};
use crate::platform::credentials::{self, Error};

/// The app's provider keys: [`ProviderKeys`] on the OS credential store, under the app's `service` (its namespace).
pub struct Keychain {
    service: String,
}

impl Keychain {
    /// `service` is the namespace of the app's keys in the store, which the app names exactly (the desktop's is
    /// `ai.sidevoice.desktop.providers`): this crate holds none of its own.
    pub fn new(service: impl Into<String>) -> Keychain {
        Keychain { service: service.into() }
    }

    /// Deletes the whole namespace, including accounts this version does not know; partial cleanup is an error. For the
    /// reset, which forgets every key the app holds.
    pub fn clear_all(&self) -> Result<(), KeyError> {
        credentials::clear_all(&self.service).map_err(KeyError::from)
    }

    /// The providers with a key in the namespace `service`, which is not the app's own but one an earlier build kept its
    /// keys in: the accounts there that are valid provider ids. `Err` when it cannot be listed.
    pub fn providers_in(service: &str) -> Result<Vec<String>, KeyError> {
        credentials::providers_in(service).map_err(KeyError::from)
    }

    /// Copy `provider`'s key from the namespace `from` to the app's own, unless that holds a key for it: `Ok(true)`
    /// copied and read back, `Ok(false)` nothing to copy. Nothing is overwritten or deleted. One key per call, each with
    /// its own admission and deadline. Both hooks run on the store's worker, even for a copy that completes after this
    /// call stopped waiting (`Unavailable` at the deadline): `may_write` right before the key is written, which happens
    /// only if it returns true; `settled` once the key is settled (written and read back equal, or nothing to copy), and
    /// the call fails unless it returns true.
    pub fn copy_key_from(
        &self,
        from: &str,
        provider: &str,
        may_write: impl FnOnce() -> bool + Send + 'static,
        settled: impl FnOnce() -> bool + Send + 'static,
    ) -> Result<bool, KeyError> {
        credentials::copy_key_from(from, &self.service, provider, may_write, settled).map_err(KeyError::from)
    }
}

impl From<Error> for KeyError {
    fn from(error: Error) -> Self {
        match error {
            Error::Unavailable => KeyError::Unavailable,
            Error::InvalidProvider => KeyError::InvalidProvider,
            Error::TooLong => KeyError::TooLong,
            Error::Failed => KeyError::Failed,
        }
    }
}

impl ProviderKeys for Keychain {
    fn valid(&self, provider: &str) -> bool {
        credentials::valid(provider)
    }

    fn read(&self, provider: &str) -> Result<Option<String>, KeyError> {
        credentials::read(&self.service, provider).map_err(KeyError::from)
    }

    fn write(&self, provider: &str, key: Option<&str>) -> Result<(), KeyError> {
        credentials::write(&self.service, provider, key).map_err(KeyError::from)
    }
}
