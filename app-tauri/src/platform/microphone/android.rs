//! Android's runtime permission `RECORD_AUDIO`, asked of the person by the plugin's Kotlin class
//! (`android/src/main/java/ai/sidevoice/app/MicrophonePlugin.kt`) through Tauri's own `checkPermissions` and
//! `requestPermissions` commands: they hold the activity result and remember a "never ask again", which the system's
//! `checkSelfPermission` alone cannot tell from "not asked yet". The same class opens the app's page of the system's
//! settings (`openSettings`), where that refusal is undone.
use super::{AccessState, Os, SETTINGS_NOT_OPENED};
use serde_json::{json, Value};
use std::{error::Error, future::Future, pin::Pin, sync::OnceLock};
use tauri::plugin::{PluginApi, PluginHandle};
use tauri::{AppHandle, Runtime};

/// The Kotlin class and the alias of the permission it declares.
const PACKAGE: &str = "ai.sidevoice.app";
const CLASS: &str = "MicrophonePlugin";
const ALIAS: &str = "microphone";
/// The class's own command that opens the app's page of the system's settings.
const OPEN_SETTINGS: &str = "openSettings";

const UNANSWERED: &str = "The system did not answer the microphone request.";

pub struct Native;
impl Os for Native {
    fn setup<R: Runtime>(api: PluginApi<R, ()>) -> Result<(), Box<dyn Error>> {
        let class = api.register_android_plugin(PACKAGE, CLASS)?;
        KOTLIN.set(Box::new(class)).map_err(|_| "the microphone plugin is set up twice")?;
        Ok(())
    }

    fn permission() -> Result<AccessState, &'static str> {
        access(&KOTLIN.get().ok_or(UNANSWERED)?.check()?)
    }

    async fn ask() -> Result<AccessState, &'static str> {
        access(&KOTLIN.get().ok_or(UNANSWERED)?.request().await?)
    }

    /// The app's page of the system's settings, where a refusal for good is undone. It is an intent, which the opener
    /// plugin (an `ACTION_VIEW` of a URI) cannot send, so the Kotlin class sends it.
    fn settings_available() -> bool {
        true
    }

    fn open_settings<R: Runtime>(_app: &AppHandle<R>) -> Result<(), &'static str> {
        KOTLIN.get().ok_or(SETTINGS_NOT_OPENED)?.open_settings()
    }
}

/// The permission commands of the loaded class, whatever runtime its handle belongs to: the platform's functions take
/// no app, so the handle waits here from the plugin's setup on.
trait Kotlin: Send + Sync {
    /// The permission states now, without prompting.
    fn check(&self) -> Result<Value, &'static str>;
    /// Prompts when the system will, and answers with the states once the person has.
    fn request(&self) -> Pin<Box<dyn Future<Output = Result<Value, &'static str>> + Send + '_>>;
    /// Shows the app's page of the system's settings.
    fn open_settings(&self) -> Result<(), &'static str>;
}

impl<R: Runtime> Kotlin for PluginHandle<R> {
    fn check(&self) -> Result<Value, &'static str> {
        // `checkPermissions` reads no argument.
        self.run_mobile_plugin("checkPermissions", ()).map_err(|_| UNANSWERED)
    }

    fn request(&self) -> Pin<Box<dyn Future<Output = Result<Value, &'static str>> + Send + '_>> {
        Box::pin(async {
            self.run_mobile_plugin_async("requestPermissions", request_arguments()).await.map_err(|_| UNANSWERED)
        })
    }

    fn open_settings(&self) -> Result<(), &'static str> {
        // `openSettings` reads no argument either, and answers with an empty object.
        self.run_mobile_plugin::<Value>(OPEN_SETTINGS, ()).map(drop).map_err(|_| SETTINGS_NOT_OPENED)
    }
}

/// What Tauri's `requestPermissions` reads: the aliases to ask for. It parses its argument and dereferences the result, so
/// an argument that is not an object (`()` is `null`) throws before the system is asked, and the person is never prompted.
fn request_arguments() -> Value {
    json!({ "permissions": [ALIAS] })
}

static KOTLIN: OnceLock<Box<dyn Kotlin>> = OnceLock::new();

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
        let class = include_str!("../../../android/src/main/java/ai/sidevoice/app/MicrophonePlugin.kt");
        assert!(class.contains(&format!("alias = \"{ALIAS}\"")), "MicrophonePlugin.kt declares no `{ALIAS}` alias");
        assert!(class.contains(&format!("package {PACKAGE}\n")) && class.contains(&format!("class {CLASS}(")));
    }

    #[test]
    fn the_settings_command_is_one_the_kotlin_class_has_and_opens_the_apps_own_page() {
        let class = include_str!("../../../android/src/main/java/ai/sidevoice/app/MicrophonePlugin.kt");
        assert!(
            class.contains(&format!("@Command\n    fun {OPEN_SETTINGS}(invoke: Invoke)")),
            "MicrophonePlugin.kt has no `{OPEN_SETTINGS}` command"
        );
        // The action and the `package:` URI of this app, not of another.
        assert!(class.contains("Settings.ACTION_APPLICATION_DETAILS_SETTINGS"));
        assert!(class.contains(r#"Uri.fromParts("package", activity.packageName, null)"#));
    }
}
