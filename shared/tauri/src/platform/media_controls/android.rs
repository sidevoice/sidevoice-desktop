//! Android: the plugin's Kotlin (`android/…/MediaControls.kt`, reached through `AppPlugin.kt`, `platform::kotlin`) is
//! told the call (active, playing while the microphone is open, its title) and hands back the controls' commands. The
//! media session is the call's foreground service's (the mobile app's `CallService`), which exists only during the call:
//! it says the call as playing or paused, Android hands it a headset's play/pause, and its notification and the lock
//! screen show it. The commands are mapped as on macOS and iOS (`crate::headset`): play → unmute, pause → toggle.
//!
//! Every command that arrives is recorded in the app log (`headset …` lines), as on the Apple systems.
use super::{CallState, CommandSink, MediaCommand, Os};
use crate::headset::media_command_action;
use crate::platform::kotlin;
use serde_json::{json, Value};
use std::sync::Mutex;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};

/// The Kotlin class's commands.
const ATTACH: &str = "attachMediaControls";
const UPDATE: &str = "updateMediaControls";

struct Headset {
    call: Mutex<CallState>,
    sink: CommandSink,
}

fn record(source: &str, action: &str) {
    sidevoice_shared::debug::debug(&format!("headset {source} → {action}"));
}

/// A command of the session arrived (`play`, `pause`), on the Kotlin side's main thread.
fn on_media_command(app: &AppHandle, command: &str) {
    let headset = app.state::<Headset>();
    let call = headset.call.lock().unwrap().clone();
    let in_call = call.ready && call.joined;
    let source = format!("session:{command}");
    match if in_call { media_command_action(command, call.mic_enabled) } else { None } {
        Some(what) => {
            (headset.sink)(MediaCommand::ToggleMute);
            record(&source, what);
        }
        None if in_call => record(&source, "already so"),
        None => record(&source, "no call: only recorded"),
    }
}

/// What the Kotlin side is told of the call: whether the session is active, whether the call plays (the microphone is
/// open) and its title (the conversation's, or none).
fn session_state(call: &CallState) -> Value {
    json!({
        "active": call.ready && call.joined,
        "playing": call.mic_enabled,
        "title": call.title.trim(),
    })
}

pub(super) struct Native;

impl Os for Native {
    const NATIVE: bool = true;

    fn attach(app: &AppHandle, sink: CommandSink) {
        app.manage(Headset { call: Mutex::default(), sink });
        let commands_app = app.clone();
        let commands = Channel::<Value>::new(move |body| {
            let message: Value = body.deserialize()?;
            if let Some(command) = message.get("command").and_then(Value::as_str) {
                on_media_command(&commands_app, command);
            }
            Ok(())
        });
        let Some(class) = kotlin::class() else { return record("attach", "no Kotlin class") };
        if let Err(error) = class.run(ATTACH, json!({ "onCommand": commands })) {
            record("attach", &error);
        }
    }

    fn update(app: &AppHandle, call: &CallState) {
        let Some(headset) = app.try_state::<Headset>() else { return };
        *headset.call.lock().unwrap() = call.clone();
        let Some(class) = kotlin::class() else { return };
        if let Err(error) = class.run(UPDATE, session_state(call)) {
            record("update", &error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_is_active_only_in_a_call_and_plays_while_the_microphone_is_open() {
        let call =
            CallState { ready: true, joined: true, mic_enabled: false, title: " Repo ".into(), now_playing_off: false };
        assert_eq!(session_state(&call), json!({"active": true, "playing": false, "title": "Repo"}));
        let idle = CallState { ready: true, ..Default::default() };
        assert_eq!(session_state(&idle)["active"], false);
    }

    #[test]
    fn the_commands_are_the_kotlin_class_s() {
        let class = include_str!("../../../android/src/main/java/ai/sidevoice/app/AppPlugin.kt");
        for command in [ATTACH, UPDATE] {
            assert!(class.contains(&format!("@Command\n    fun {command}(invoke: Invoke)")), "{command}");
        }
        assert!(class.contains("lateinit var onCommand: Channel"));
        let controls = include_str!("../../../android/src/main/java/ai/sidevoice/app/MediaControls.kt");
        assert!(
            controls.contains("fun command(name: String)")
                && controls.contains("fun update(active: Boolean, playing: Boolean, title: String)")
        );
    }
}
