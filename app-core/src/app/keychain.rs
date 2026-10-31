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
