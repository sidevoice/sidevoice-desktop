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
use crate::{CallDetails, CallEvent, CallEvents, CallServiceError};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartArgs<'a> {
    title: &'a str,
    on_event: Channel<CallEvent>,
}

#[derive(Serialize)]
struct UpdateArgs<'a> {
    title: &'a str,
    muted: bool,
}

pub struct Native;

impl Os for Native {
    // Telecom making the call active switches the audio mode and the route (to the loudspeaker), and AAudio cuts off a
    // stream open across that: the service reports `audio` once both are done, or at once when Telecom refuses the call.
    const ACTIVATES_AUDIO: bool = true;

    fn register<R: Runtime, C: DeserializeOwned>(
        api: PluginApi<R, C>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        kotlin::register(api)
    }

    fn start<R: Runtime>(
        app: &AppHandle<R>,
        details: &CallDetails,
        events: CallEvents,
    ) -> Result<(), CallServiceError> {
        // The service's reports come over a channel, as JSON.
        let on_event = Channel::<CallEvent>::new(move |body| {
            events(
                body.deserialize::<CallEvent>()
                    .map_err(tauri::Error::Json)?,
            );
            Ok(())
        });
        run(
            app,
            "startCallService",
            StartArgs {
                title: &details.title,
                on_event,
            },
        )
    }

    fn update<R: Runtime>(
        app: &AppHandle<R>,
        details: &CallDetails,
    ) -> Result<(), CallServiceError> {
        run(
            app,
            "updateCallService",
            UpdateArgs {
                title: &details.title,
                muted: details.muted,
            },
        )
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
