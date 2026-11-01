use serde::de::DeserializeOwned;
use tauri::{ipc::Channel, plugin::PluginApi, AppHandle, Runtime};

use super::Os;
use crate::{CallEvent, CallServiceError};

/// An OS the mobile app does not run on (its crates build here only so that the repository's checks cover them).
pub struct Native;

impl Os for Native {
    fn register<R: Runtime, C: DeserializeOwned>(
        _api: PluginApi<R, C>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }

    fn start<R: Runtime>(
        _app: &AppHandle<R>,
        _events: Channel<CallEvent>,
    ) -> Result<(), CallServiceError> {
        Err(CallServiceError::Unsupported)
    }

    fn stop<R: Runtime>(_app: &AppHandle<R>) -> Result<(), CallServiceError> {
        Err(CallServiceError::Unsupported)
    }
}
