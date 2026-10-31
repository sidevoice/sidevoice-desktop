//! What this app gives sidevoice-app-core's application host (`AppHost`): its reset as the admission to work that uses
//! the app's data, and the OS credential store as where provider keys are kept. (Where the call's events go is the shared
//! plugin's: the channel the room page opens.)

use sidevoice_app_core::app::{Admission, KeyError, Lease, ProviderKeys};
use tauri::AppHandle;

use crate::platform::credentials;

/// Work is admitted while no reset is under way, and holds the reset off while it lasts (`data::Permit`).
pub(crate) struct ResetAdmission(pub(crate) AppHandle);

impl Admission for ResetAdmission {
    fn admit(&self) -> Option<Lease> {
        crate::data::admit(&self.0).map(Lease::new)
    }
}

/// The provider keys are the OS credential store's (docs/CREDENTIALS.md).
pub(crate) struct Keys;

fn key_error(error: credentials::Error) -> KeyError {
    match error {
        credentials::Error::Unavailable => KeyError::Unavailable,
        credentials::Error::InvalidProvider => KeyError::InvalidProvider,
        credentials::Error::TooLong => KeyError::TooLong,
        credentials::Error::Failed => KeyError::Failed,
    }
}

impl ProviderKeys for Keys {
    fn valid(&self, provider: &str) -> bool {
        credentials::valid(provider)
    }

    fn read(&self, provider: &str) -> Result<Option<String>, KeyError> {
        credentials::read(provider).map_err(key_error)
    }

    fn write(&self, provider: &str, key: Option<&str>) -> Result<(), KeyError> {
        credentials::write(provider, key).map_err(key_error)
    }
}
