//! Android's runtime permission `RECORD_AUDIO`, asked of the person by the plugin's Kotlin class
//! (`android/src/main/java/ai/sidevoice/app/MicrophonePlugin.kt`) through Tauri's own `checkPermissions` and
//! `requestPermissions` commands: they hold the activity result and remember a "never ask again", which the system's
//! `checkSelfPermission` alone cannot tell from "not asked yet".
use super::{AccessState, Os};
use serde_json::Value;
use std::{error::Error, future::Future, pin::Pin, sync::OnceLock};
use tauri::plugin::{PluginApi, PluginHandle};
use tauri::Runtime;

/// The Kotlin class and the alias of the permission it declares.
const PACKAGE: &str = "ai.sidevoice.app";
const CLASS: &str = "MicrophonePlugin";
const ALIAS: &str = "microphone";

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
}

/// The permission commands of the loaded class, whatever runtime its handle belongs to: the platform's functions take
/// no app, so the handle waits here from the plugin's setup on.
trait Kotlin: Send + Sync {
    /// The permission states now, without prompting.
    fn check(&self) -> Result<Value, &'static str>;
    /// Prompts when the system will, and answers with the states once the person has.
    fn request(&self) -> Pin<Box<dyn Future<Output = Result<Value, &'static str>> + Send + '_>>;
}

impl<R: Runtime> Kotlin for PluginHandle<R> {
    fn check(&self) -> Result<Value, &'static str> {
        self.run_mobile_plugin("checkPermissions", ()).map_err(|_| UNANSWERED)
    }

    fn request(&self) -> Pin<Box<dyn Future<Output = Result<Value, &'static str>> + Send + '_>> {
        Box::pin(async { self.run_mobile_plugin_async("requestPermissions", ()).await.map_err(|_| UNANSWERED) })
    }
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
