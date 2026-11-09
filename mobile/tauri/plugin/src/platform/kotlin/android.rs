//! Android: the Kotlin plugin (`android/…/MobilePlugin.kt`) every Android body of this crate reaches the system through.
use serde::{de::DeserializeOwned, Serialize};
use tauri::{
    plugin::{mobile::PluginInvokeError, PluginApi, PluginHandle},
    AppHandle, Manager, Runtime,
};

/// The Kotlin plugin's package, and its class.
const PACKAGE: &str = "ai.sidevoice.mobile.plugin";
const CLASS: &str = "MobilePlugin";

/// The handle of the Kotlin plugin, kept in the app's state.
struct Plugin<R: Runtime>(PluginHandle<R>);

/// Registers the Kotlin plugin when the app starts.
pub fn register<R: Runtime, C: DeserializeOwned>(
    api: PluginApi<R, C>,
) -> Result<(), Box<dyn std::error::Error>> {
    let handle = api.register_android_plugin(PACKAGE, CLASS)?;
    api.app().manage(Plugin(handle));
    Ok(())
}

/// Runs the Kotlin plugin's `command` with `args`, and waits for its answer (`T`, `()` for none). `None` when the plugin is not
/// set up. The command runs on Android's main thread: never call this from there, it would wait on itself.
pub fn run<R: Runtime, T: DeserializeOwned>(
    app: &AppHandle<R>,
    command: &str,
    args: impl Serialize,
) -> Option<Result<T, PluginInvokeError>> {
    let plugin = app.try_state::<Plugin<R>>()?;
    Some(plugin.0.run_mobile_plugin::<T>(command, args))
}
