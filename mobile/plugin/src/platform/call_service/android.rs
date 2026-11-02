use serde::{de::DeserializeOwned, Serialize};
use tauri::{
    ipc::Channel,
    plugin::{
        mobile::{ErrorResponse, PluginInvokeError},
        PluginApi,
    },
    AppHandle, Runtime,
};

use super::Os;
use crate::platform::kotlin;
use crate::{CallEvent, CallServiceError};

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
        kotlin::register(api)
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

    fn refocus<R: Runtime>(app: &AppHandle<R>) -> Result<(), CallServiceError> {
        run(app, "requestCallFocus", ())
    }
}

fn run<R: Runtime>(
    app: &AppHandle<R>,
    command: &str,
    args: impl Serialize,
) -> Result<(), CallServiceError> {
    kotlin::run(app, command, args)
        .ok_or_else(|| CallServiceError::Unreachable("the plugin is not set up".into()))?
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
