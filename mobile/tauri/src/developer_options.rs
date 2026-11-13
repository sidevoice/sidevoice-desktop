//! The developer options of the phone (`window.__sidevoiceHost.host.debug`, docs/BRIDGE.md → "The developer options"): in a
//! nightly or a dev build, the switches the room's hidden "Developer options" section turns. Off by default, kept in a small
//! file of the app's own files, and absent (every switch off) in a release.
//!
//! One switch: `systemCall` ("Treat calls as system calls", D-NO-CALLKIT-DEFAULT). Off, the room's call is the app's
//! own: no CallKit provider is made on iOS, and Android's call service registers nothing with Telecom (docs/MOBILE.md → "The
//! system's call"). On, the call is a system's call (CallKit, Core-Telecom), for testing with a headset or in a car.
//!
//! And on iOS one choice: `callAudio` ("Call audio mode (test)", [`CallAudio`]), the audio session's mode, for an A/B test on
//! a device of how loud the loudspeaker plays (docs/MOBILE.md → "The call audio mode test"). Voice chat, the declaration
//! the app always made, by default.
use std::{
    fs, io,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};

/// The options this phone has, in the order the page lists them (`host.debug.available`). The call audio mode is offered
/// only where the audio session's mode can be chosen (iOS's; Android has no such session).
pub fn available() -> &'static [&'static str] {
    if crate::platform::audio_session::mode_selectable() {
        &["systemCall", "callAudio"]
    } else {
        &["systemCall"]
    }
}

/// How the audio session is declared (iOS), the developer option "Call audio mode (test)": whether the session's mode is
/// what keeps the loudspeaker quiet. Every choice is play-and-record with the loudspeaker rather than the receiver and a
/// Bluetooth hands-free headset allowed (the category and the options a voice call needs: it records); only the mode
/// differs (Apple's `AVAudioSession.Mode`). The voice's own echo cancellation is the same in all of them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CallAudio {
    /// `.voiceChat` always: the declaration the app always made (`call::CALL_AUDIO`), with the system's voice processing
    /// and its call volume.
    #[default]
    VoiceChat,
    /// `.videoChat` always: the system's voice processing too, tuned for a video call.
    VideoChat,
    /// `.default` always: no processing of the system's, and its media volume.
    Default,
    /// `.default` at the app's start and for every voice call outside the room's call (a try in Settings), `.voiceChat` only
    /// while the room holds a call.
    VoiceChatInCalls,
}

impl CallAudio {
    const ALL: [CallAudio; 4] = [
        CallAudio::VoiceChat,
        CallAudio::VideoChat,
        CallAudio::Default,
        CallAudio::VoiceChatInCalls,
    ];

    /// Its name, as the page and the log have it.
    pub fn name(self) -> &'static str {
        match self {
            CallAudio::VoiceChat => "voiceChat",
            CallAudio::VideoChat => "videoChat",
            CallAudio::Default => "default",
            CallAudio::VoiceChatInCalls => "voiceChatInCalls",
        }
    }

    fn named(name: &str) -> Option<CallAudio> {
        Self::ALL.into_iter().find(|choice| choice.name() == name)
    }

    fn index(self) -> u8 {
        match self {
            CallAudio::VoiceChat => 0,
            CallAudio::VideoChat => 1,
            CallAudio::Default => 2,
            CallAudio::VoiceChatInCalls => 3,
        }
    }

    fn of_index(index: u8) -> CallAudio {
        Self::ALL
            .get(usize::from(index))
            .copied()
            .unwrap_or_default()
    }
}

/// The record's file name, in the app's own files.
const FILE: &str = "developer-options.json";

/// Whether a build of `channel` offers the developer options: a nightly or a dev build, never a release (the same rule as
/// the desktop's `debug_options::offered`).
pub fn offered(channel: &str) -> bool {
    channel != "release"
}

/// What is kept: every switch absent from the record is off, and every choice its default.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Kept {
    system_call: bool,
    call_audio: CallAudio,
}

/// The app's developer options: what the page changed, read by the call.
pub struct DeveloperOptions {
    /// Where they are kept; `None` in a build that does not offer them.
    file: Option<PathBuf>,
    system_call: AtomicBool,
    /// The [`CallAudio`] chosen, by its index.
    call_audio: AtomicU8,
}

impl DeveloperOptions {
    /// The options of the app's build `channel`, read from its own files: a record that is missing or unreadable is every
    /// switch off, and a release has none whatever a record says.
    pub fn of_app<R: Runtime>(app: &AppHandle<R>, channel: &str) -> tauri::Result<Self> {
        Ok(Self::new(&crate::own_files(app)?, channel))
    }

    fn new(files: &std::path::Path, channel: &str) -> Self {
        if !offered(channel) {
            return Self {
                file: None,
                system_call: AtomicBool::new(false),
                call_audio: AtomicU8::new(CallAudio::default().index()),
            };
        }
        let file = files.join(FILE);
        let kept = fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Kept>(&bytes).ok())
            .unwrap_or_default();
        Self {
            file: Some(file),
            system_call: AtomicBool::new(kept.system_call),
            call_audio: AtomicU8::new(kept.call_audio.index()),
        }
    }

    /// Whether the room's call is a system's call (CallKit, Telecom). Off unless the person turned it on.
    pub fn system_call(&self) -> bool {
        self.system_call.load(Ordering::Relaxed)
    }

    /// How the audio session is declared (iOS): voice chat unless the person chose another mode, and always where the
    /// choice is not offered (Android, a release).
    pub fn call_audio(&self) -> CallAudio {
        if !available().contains(&"callAudio") {
            return CallAudio::default();
        }
        CallAudio::of_index(self.call_audio.load(Ordering::Relaxed))
    }

    /// The options as the page reads them (`host.debug`'s state), or `None` where they are not offered. A choice this phone
    /// lacks is not in it.
    pub fn state(&self) -> Option<Value> {
        self.file.as_ref().map(|_| {
            let mut state = json!({ "available": available(), "systemCall": self.system_call() });
            if available().contains(&"callAudio") {
                state["callAudio"] = json!(self.call_audio().name());
            }
            state
        })
    }

    /// Turns the switch `name` on or off, and keeps it. Refused for a switch this phone lacks and in a build without the
    /// options; the new state is what the page reads back.
    pub fn set(&self, name: &str, on: bool) -> Result<Value, String> {
        match name {
            "systemCall" => {
                self.keep(Kept {
                    system_call: on,
                    call_audio: self.call_audio(),
                })?;
                self.system_call.store(on, Ordering::Relaxed);
            }
            _ => return Err(format!("not a developer switch: {name}")),
        }
        Ok(self.state().unwrap_or(Value::Null))
    }

    /// Chooses `value` for the option `name`, and keeps it. Refused for a choice this phone lacks, a value the choice does
    /// not have, and in a build without the options; the new state is what the page reads back.
    pub fn choose(&self, name: &str, value: &str) -> Result<Value, String> {
        match name {
            "callAudio" if available().contains(&"callAudio") => {
                let choice = CallAudio::named(value)
                    .ok_or_else(|| format!("not a call audio mode: {value}"))?;
                self.keep(Kept {
                    system_call: self.system_call(),
                    call_audio: choice,
                })?;
                self.call_audio.store(choice.index(), Ordering::Relaxed);
            }
            _ => return Err(format!("not a developer choice: {name}")),
        }
        Ok(self.state().unwrap_or(Value::Null))
    }

    /// Writes `kept` as the record, in a build that offers the options.
    fn keep(&self, kept: Kept) -> Result<(), String> {
        let file = self
            .file
            .as_ref()
            .ok_or("The developer options are not in this build.")?;
        if let Some(directory) = file.parent() {
            fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        }
        write(file, &kept).map_err(|error| error.to_string())
    }
}

/// The record written whole to a temporary file beside it and renamed over the old one, so that a crash leaves the old
/// record or the new one.
fn write(file: &std::path::Path, kept: &Kept) -> io::Result<()> {
    let temporary = file.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec(kept)?)?;
    fs::rename(&temporary, file)
}

/// Whether the room's call is a system's call now: the app's developer option, off when it is not managed (a test) or not
/// offered.
pub fn system_call<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<DeveloperOptions>()
        .is_some_and(|options| options.system_call())
}

/// How the audio session is declared now: the app's developer option, voice chat when it is not managed (a test) or not
/// offered.
pub fn call_audio<R: Runtime>(app: &AppHandle<R>) -> CallAudio {
    app.try_state::<DeveloperOptions>()
        .map(|options| options.call_audio())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The state with `system_call`, as this phone has it (the call audio mode at its default where offered).
    fn state(system_call: bool) -> Value {
        let mut state = json!({"available": available(), "systemCall": system_call});
        if available().contains(&"callAudio") {
            state["callAudio"] = json!("voiceChat");
        }
        state
    }

    #[test]
    fn a_release_offers_none_and_a_nightly_or_a_dev_build_offers_them() {
        assert!(!offered("release"));
        assert!(offered("nightly"));
        assert!(offered("dev"));
    }

    #[test]
    fn the_call_is_the_app_s_own_until_the_person_turns_it_on() {
        let files = tempfile::tempdir().unwrap();
        let options = DeveloperOptions::new(files.path(), "nightly");
        assert!(!options.system_call());
        assert_eq!(options.state(), Some(state(false)));
    }

    #[test]
    fn a_switch_is_kept_across_a_restart_and_turned_off_again() {
        let files = tempfile::tempdir().unwrap();
        let options = DeveloperOptions::new(files.path(), "dev");
        assert_eq!(options.set("systemCall", true), Ok(state(true)));
        assert!(options.system_call());
        assert!(DeveloperOptions::new(files.path(), "dev").system_call());
        options.set("systemCall", false).unwrap();
        assert!(!DeveloperOptions::new(files.path(), "dev").system_call());
    }

    #[test]
    fn a_release_ignores_a_kept_switch_and_refuses_to_change_one() {
        let files = tempfile::tempdir().unwrap();
        DeveloperOptions::new(files.path(), "nightly")
            .set("systemCall", true)
            .unwrap();
        let release = DeveloperOptions::new(files.path(), "release");
        assert!(!release.system_call());
        assert_eq!(release.state(), None);
        assert!(release.set("systemCall", true).is_err());
        assert!(!release.system_call());
    }

    #[test]
    fn what_is_not_a_switch_is_refused_and_an_unreadable_record_is_all_off() {
        let files = tempfile::tempdir().unwrap();
        let options = DeveloperOptions::new(files.path(), "nightly");
        assert!(options.set("inspectable", true).is_err());
        assert!(options.set("callAudio", true).is_err());
        fs::write(files.path().join(FILE), b"not json").unwrap();
        assert!(!DeveloperOptions::new(files.path(), "nightly").system_call());
    }

    #[test]
    fn the_call_audio_mode_is_voice_chat_until_another_is_chosen_and_is_kept_with_the_switch() {
        let files = tempfile::tempdir().unwrap();
        if !available().contains(&"callAudio") {
            // A phone without the choice refuses it and keeps voice chat.
            let options = DeveloperOptions::new(files.path(), "nightly");
            assert!(options.choose("callAudio", "default").is_err());
            assert_eq!(options.call_audio(), CallAudio::VoiceChat);
            return;
        }
        let options = DeveloperOptions::new(files.path(), "nightly");
        assert_eq!(options.call_audio(), CallAudio::VoiceChat);
        options.set("systemCall", true).unwrap();
        assert_eq!(
            options.choose("callAudio", "voiceChatInCalls"),
            Ok(
                json!({"available": available(), "systemCall": true, "callAudio": "voiceChatInCalls"})
            )
        );
        let again = DeveloperOptions::new(files.path(), "nightly");
        assert_eq!(again.call_audio(), CallAudio::VoiceChatInCalls);
        assert!(again.system_call(), "a choice keeps the switch");
        assert!(options.choose("callAudio", "spokenAudio").is_err());
        assert!(options.choose("systemCall", "on").is_err());
        assert_eq!(options.call_audio(), CallAudio::VoiceChatInCalls);
        let release = DeveloperOptions::new(files.path(), "release");
        assert_eq!(release.call_audio(), CallAudio::VoiceChat);
        assert!(release.choose("callAudio", "default").is_err());
    }

    #[test]
    fn every_call_audio_mode_is_found_by_its_name() {
        for choice in CallAudio::ALL {
            assert_eq!(CallAudio::named(choice.name()), Some(choice));
            assert_eq!(CallAudio::of_index(choice.index()), choice);
        }
    }
}
