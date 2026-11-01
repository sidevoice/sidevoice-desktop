//! Android: the plugin's Kotlin class (`android/src/main/java/ai/sidevoice/app/AppPlugin.kt`), which the concerns that
//! need Android's own APIs reach (the microphone's permission, the media session). Tauri keeps one Kotlin class per
//! plugin, by the plugin's name, so the concerns share this one: the plugin's setup registers it (through the
//! microphone's, which runs first and always), and each concern calls its own commands on it.
use serde_json::Value;
use std::{error::Error, future::Future, pin::Pin, sync::OnceLock};
use tauri::plugin::{PluginApi, PluginHandle};
use tauri::Runtime;

/// The Kotlin class.
pub const PACKAGE: &str = "ai.sidevoice.app";
pub const CLASS: &str = "AppPlugin";

/// The loaded class, whatever runtime its handle belongs to: the platform's functions take no app, so the handle waits
/// here from the plugin's setup on.
pub trait Kotlin: Send + Sync {
    /// A command of the class, and its answer.
    fn run(&self, command: &str, args: Value) -> Result<Value, String>;
    /// A command of the class that answers later (a prompt the person answers).
    fn run_async(&self, command: &str, args: Value)
        -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + '_>>;
}

impl<R: Runtime> Kotlin for PluginHandle<R> {
    fn run(&self, command: &str, args: Value) -> Result<Value, String> {
        self.run_mobile_plugin(command, args).map_err(|e| e.to_string())
    }

    fn run_async(
        &self,
        command: &str,
        args: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + '_>> {
        let command = command.to_owned();
        Box::pin(async move { self.run_mobile_plugin_async(command, args).await.map_err(|e| e.to_string()) })
    }
}

static CLASS_HANDLE: OnceLock<Box<dyn Kotlin>> = OnceLock::new();

/// Loads the class, once, at the plugin's setup.
pub fn register<R: Runtime>(api: &PluginApi<R, ()>) -> Result<(), Box<dyn Error>> {
    let class = api.register_android_plugin(PACKAGE, CLASS)?;
    CLASS_HANDLE.set(Box::new(class)).map_err(|_| "the plugin's Kotlin class is set up twice")?;
    Ok(())
}

/// The loaded class; `None` before the plugin's setup.
pub fn class() -> Option<&'static dyn Kotlin> {
    CLASS_HANDLE.get().map(|class| &**class)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_class_is_the_one_the_kotlin_file_declares() {
        let class = include_str!("../../android/src/main/java/ai/sidevoice/app/AppPlugin.kt");
        assert!(class.contains(&format!("package {PACKAGE}\n")) && class.contains(&format!("class {CLASS}(")));
    }
}
