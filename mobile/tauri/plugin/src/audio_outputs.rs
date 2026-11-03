//! The phone's built-in outputs by type (docs/MOBILE.md → "The outputs' names"). Android returns the phone's model as the
//! product name of the earpiece, the loudspeaker and the rest of the built-in outputs, so the picker listed three entries
//! with the model number; the Kotlin plugin names them by what they are, in the person's language, and the app words the
//! devices the voice lists with them. An external device (a headset, USB, Bluetooth) keeps the name it gives itself.
use serde::Deserialize;
use tauri::{AppHandle, Runtime};

use crate::platform::audio_outputs as platform;

/// The id the voice lists a device by on Android: cpal's AAudio host and the system's `AudioDeviceInfo.getId()`.
const ID_PREFIX: &str = "aaudio:";

/// A built-in output of the phone: the system's id of it, and its name in the person's language.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BuiltInOutput {
    pub id: i32,
    pub label: String,
}

/// The phone's built-in outputs, named. Empty where the system names them already, and when the plugin does not answer (the
/// devices then keep the system's names).
pub fn built_in_outputs<R: Runtime>(app: &AppHandle<R>) -> Vec<BuiltInOutput> {
    platform::built_in(app)
}

/// The name of the voice's device `device_id` among `outputs`: only for a built-in output of this phone.
pub fn label_of<'a>(outputs: &'a [BuiltInOutput], device_id: &str) -> Option<&'a str> {
    let id: i32 = device_id.strip_prefix(ID_PREFIX)?.parse().ok()?;
    outputs
        .iter()
        .find(|output| output.id == id)
        .map(|output| output.label.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outputs() -> Vec<BuiltInOutput> {
        vec![
            BuiltInOutput {
                id: 2,
                label: "Earpiece".into(),
            },
            BuiltInOutput {
                id: 3,
                label: "Speaker".into(),
            },
        ]
    }

    #[test]
    fn a_built_in_output_is_known_by_the_system_s_id_in_the_voice_s() {
        assert_eq!(label_of(&outputs(), "aaudio:3"), Some("Speaker"));
        assert_eq!(label_of(&outputs(), "aaudio:2"), Some("Earpiece"));
    }

    #[test]
    fn anything_else_keeps_its_own_name() {
        // An external device, the system's default (-1), another host's id and what is not an id.
        for id in [
            "aaudio:7",
            "aaudio:-1",
            "alsa:3",
            "aaudio:",
            "aaudio:x",
            "3",
            "",
        ] {
            assert_eq!(label_of(&outputs(), id), None, "{id}");
        }
    }

    #[test]
    fn the_answer_of_the_plugin_is_read() {
        let answer: Vec<BuiltInOutput> =
            serde_json::from_str(r#"[{"id":3,"label":"Altavoz"}]"#).unwrap();
        assert_eq!(label_of(&answer, "aaudio:3"), Some("Altavoz"));
    }
}
