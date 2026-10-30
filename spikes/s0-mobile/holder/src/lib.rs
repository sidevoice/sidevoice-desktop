//! S0 spike (sidevoice-desktop#290): the native audio holder of run N, as a Tauri mobile plugin.
//!
//! iOS: `ios/Sources/HolderPlugin.swift` (AVAudioSession playAndRecord, an AVAudioEngine input tap and a silent
//! output). Android: `android/src/main/java/HolderPlugin.kt` and `HolderService.kt` (a foreground service of type
//! microphone with AudioRecord and a silent AudioTrack). Each writes its own timestamped log, `holder.jsonl`, in the
//! run directory the app hands it. Only the app's Rust calls it; no page reaches it directly.
use serde::de::DeserializeOwned;
use serde_json::Value;
use tauri::{
    plugin::{Builder, PluginApi, PluginHandle, TauriPlugin},
    Manager, Runtime,
};

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_s0_holder);

/// The native side, for the app's own Rust code.
pub struct Holder<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> Holder<R> {
    /// The directory the phone lets the person take files from (Android: the app's external files directory, which
    /// `adb pull` reads; iOS: Documents, shown in the Files app).
    pub fn log_dir(&self) -> Result<String, String> {
        let answer: Value = self
            .0
            .run_mobile_plugin("logDir", Value::Null)
            .map_err(|e| e.to_string())?;
        answer
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "logDir answered no path".into())
    }

    /// Starts logging the phone's lock, screen, app-state and network events to `dir/system.jsonl` (both runs).
    pub fn watch(&self, dir: &str) -> Result<Value, String> {
        self.call("watch", serde_json::json!({ "dir": dir }))
    }

    pub fn unwatch(&self) -> Result<Value, String> {
        self.call("unwatch", Value::Null)
    }

    /// Starts holding the audio (asking for the microphone first), logging to `dir/holder.jsonl`.
    pub fn start(&self, dir: &str) -> Result<Value, String> {
        self.call("start", serde_json::json!({ "dir": dir }))
    }

    pub fn stop(&self) -> Result<Value, String> {
        self.call("stop", Value::Null)
    }

    pub fn status(&self) -> Result<Value, String> {
        self.call("status", Value::Null)
    }

    fn call(&self, command: &str, args: Value) -> Result<Value, String> {
        self.0.run_mobile_plugin(command, args).map_err(|e| e.to_string())
    }
}

pub trait HolderExt<R: Runtime> {
    fn holder(&self) -> &Holder<R>;
}

impl<R: Runtime, T: Manager<R>> HolderExt<R> for T {
    fn holder(&self) -> &Holder<R> {
        self.state::<Holder<R>>().inner()
    }
}

fn register<R: Runtime, C: DeserializeOwned>(api: PluginApi<R, C>) -> Result<Holder<R>, Box<dyn std::error::Error>> {
    #[cfg(target_os = "android")]
    let handle = api.register_android_plugin("ai.sidevoice.spike.holder", "HolderPlugin")?;
    #[cfg(target_os = "ios")]
    let handle = api.register_ios_plugin(init_plugin_s0_holder)?;
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let handle: PluginHandle<R> = {
        let _ = api;
        return Err("the S0 holder runs on phones only".into());
    };
    Ok(Holder(handle))
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("s0-holder")
        .setup(|app, api| {
            app.manage(register(api)?);
            Ok(())
        })
        .build()
}
