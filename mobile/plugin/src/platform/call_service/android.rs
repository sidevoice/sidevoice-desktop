use serde::{de::DeserializeOwned, Serialize};
use tauri::{
    ipc::Channel,
    plugin::{
        mobile::{ErrorResponse, PluginInvokeError},
        PluginApi, PluginHandle,
    },
    AppHandle, Manager, Runtime,
};

use super::Os;
use crate::{CallEvent, CallServiceError};

/// The Kotlin plugin's package, and its class (`android/src/main/java/ai/sidevoice/mobile/plugin/MobilePlugin.kt`).
const PACKAGE: &str = "ai.sidevoice.mobile.plugin";
const CLASS: &str = "MobilePlugin";

/// The handle of the Kotlin plugin, kept in the app's state.
struct Plugin<R: Runtime>(PluginHandle<R>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartArgs {
    on_event: Channel<CallEvent>,
}

pub struct Native;

impl Os for Native {
    fn register<R: Runtime, C: DeserializeOwned>(
        api: PluginApi<R, C>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let handle = api.register_android_plugin(PACKAGE, CLASS)?;
        api.app().manage(Plugin(handle));
        Ok(())
    }

    fn start<R: Runtime>(
        app: &AppHandle<R>,
        events: Channel<CallEvent>,
    ) -> Result<(), CallServiceError> {
        run(app, "startCallService", StartArgs { on_event: events })
    }

    fn stop<R: Runtime>(app: &AppHandle<R>) -> Result<(), CallServiceError> {
        run(app, "stopCallService", ())
    }
}

fn run<R: Runtime>(
    app: &AppHandle<R>,
    command: &str,
    args: impl Serialize,
) -> Result<(), CallServiceError> {
    let plugin = app
        .try_state::<Plugin<R>>()
        .ok_or_else(|| CallServiceError::Unreachable("the plugin is not set up".into()))?;
    plugin
        .0
        .run_mobile_plugin::<()>(command, args)
        .map_err(|error| match error {
            PluginInvokeError::InvokeRejected(ErrorResponse { code, message, .. }) => {
                CallServiceError::Refused {
                    code,
                    message: message.unwrap_or_default(),
                }
            }
            other => CallServiceError::Unreachable(other.to_string()),
        })
}
