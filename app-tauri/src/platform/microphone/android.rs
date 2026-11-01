//! Android's runtime permission `RECORD_AUDIO`, asked of the person by the plugin's Kotlin class
//! (`android/src/main/java/ai/sidevoice/app/AppPlugin.kt`, `platform::kotlin`) through Tauri's own `checkPermissions` and
//! `requestPermissions` commands: they hold the activity result and remember a "never ask again", which the system's
//! `checkSelfPermission` alone cannot tell from "not asked yet". The same class opens the app's page of the system's
//! settings (`openSettings`), where that refusal is undone.
use super::{AccessState, Os, SETTINGS_NOT_OPENED};
use crate::platform::kotlin;
use serde_json::{json, Value};
use std::error::Error;
use tauri::plugin::PluginApi;
use tauri::{AppHandle, Runtime};

/// The alias of the permission the Kotlin class declares.
const ALIAS: &str = "microphone";
/// The class's own command that opens the app's page of the system's settings.
const OPEN_SETTINGS: &str = "openSettings";

const UNANSWERED: &str = "The system did not answer the microphone request.";

pub struct Native;
impl Os for Native {
    /// The plugin's Kotlin class is loaded here, for every concern that reaches it (`platform::kotlin`).
    fn setup<R: Runtime>(api: PluginApi<R, ()>) -> Result<(), Box<dyn Error>> {
        kotlin::register(&api)
    }

    fn permission() -> Result<AccessState, &'static str> {
        // `checkPermissions` reads no argument.
        let class = kotlin::class().ok_or(UNANSWERED)?;
        access(&class.run("checkPermissions", Value::Null).map_err(|_| UNANSWERED)?)
    }

    async fn ask() -> Result<AccessState, &'static str> {
        let class = kotlin::class().ok_or(UNANSWERED)?;
        access(&class.run_async("requestPermissions", request_arguments()).await.map_err(|_| UNANSWERED)?)
    }

    /// The app's page of the system's settings, where a refusal for good is undone. It is an intent, which the opener
    /// plugin (an `ACTION_VIEW` of a URI) cannot send, so the Kotlin class sends it.
    fn settings_available() -> bool {
        true
    }

    fn open_settings<R: Runtime>(_app: &AppHandle<R>) -> Result<(), &'static str> {
        // `openSettings` reads no argument either, and answers with an empty object.
        let class = kotlin::class().ok_or(SETTINGS_NOT_OPENED)?;
        class.run(OPEN_SETTINGS, Value::Null).map(drop).map_err(|_| SETTINGS_NOT_OPENED)
    }
}

/// What Tauri's `requestPermissions` reads: the aliases to ask for. It parses its argument and dereferences the result, so
/// an argument that is not an object (`()` is `null`) throws before the system is asked, and the person is never prompted.
fn request_arguments() -> Value {
    json!({ "permissions": [ALIAS] })
}

/// Tauri's permission states: `prompt` (not asked yet) and `prompt-with-rationale` (refused once, the system will ask
/// again) are the page's `prompt`; a refusal that stands is `denied`.
fn access(states: &Value) -> Result<AccessState, &'static str> {
    match states.get(ALIAS).and_then(Value::as_str) {
        Some("granted") => Ok(AccessState::Granted),
        Some("denied") => Ok(AccessState::Denied),
        Some("prompt" | "prompt-with-rationale") => Ok(AccessState::NotDetermined),
        _ => Err(UNANSWERED),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_names_the_alias_in_an_object() {
        assert_eq!(request_arguments().to_string(), r#"{"permissions":["microphone"]}"#);
    }

    #[test]
    fn the_alias_is_the_one_the_kotlin_class_declares() {
        let class = include_str!("../../../android/src/main/java/ai/sidevoice/app/AppPlugin.kt");
        assert!(class.contains(&format!("alias = \"{ALIAS}\"")), "AppPlugin.kt declares no `{ALIAS}` alias");
    }

    #[test]
    fn the_settings_command_is_one_the_kotlin_class_has_and_opens_the_apps_own_page() {
        let class = include_str!("../../../android/src/main/java/ai/sidevoice/app/AppPlugin.kt");
        assert!(
            class.contains(&format!("@Command\n    fun {OPEN_SETTINGS}(invoke: Invoke)")),
            "AppPlugin.kt has no `{OPEN_SETTINGS}` command"
        );
        // The action and the `package:` URI of this app, not of another.
        assert!(class.contains("Settings.ACTION_APPLICATION_DETAILS_SETTINGS"));
        assert!(class.contains(r#"Uri.fromParts("package", activity.packageName, null)"#));
    }
}
