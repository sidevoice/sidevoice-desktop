//! What this app gives sidevoice-app-core's application host (`AppHost`): its reset as the admission to work that uses
//! the app's data, the room page as where the call's events go, and the OS credential store as where provider keys are
//! kept.

use serde_json::Value;
use sidevoice_app_core::app::{Admission, EventSink, KeyError, Lease, ProviderKeys};
use tauri::AppHandle;

use crate::platform::credentials;

/// Work is admitted while no reset is under way, and holds the reset off while it lasts (`data::Permit`).
pub(crate) struct ResetAdmission(pub(crate) AppHandle);

impl Admission for ResetAdmission {
    fn admit(&self) -> Option<Lease> {
        crate::data::admit(&self.0).map(Lease::new)
    }
}

/// The call's events reach the room page (`voice::to_page`).
pub(crate) struct RoomEvents(pub(crate) AppHandle);

impl EventSink for RoomEvents {
    fn event(&self, event: Value) {
        crate::voice::to_page(&self.0, &event);
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
