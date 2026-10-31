use serde::de::DeserializeOwned;
use tauri::{ipc::Channel, plugin::PluginApi, AppHandle, Runtime};

use super::Os;
use crate::{CallEvent, CallServiceError};

/// iOS has no service to start: a call stays alive in the background through the `audio` background mode, which is
/// configuration (`UIBackgroundModes` in mobile/Info.ios.plist), not code.
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
        Ok(())
    }

    fn stop<R: Runtime>(_app: &AppHandle<R>) -> Result<(), CallServiceError> {
        Ok(())
    }
}
