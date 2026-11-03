//! What the phone's outputs are (docs/MOBILE.md → "The outputs' names"). Android returns the phone's model as the product name
//! of every built-in output, so the picker listed three entries with the model number, one of them (the telephony path) that
//! plays nothing. The Kotlin plugin tells each output's kind by its type; the app words the kinds, in the page's language
//! (`mobile/tauri/src/output_names.rs`). An external device (a headset, USB, Bluetooth) names itself.
use serde::Deserialize;
use tauri::{AppHandle, Runtime};

use crate::platform::audio_outputs as platform;

/// The id the voice lists a device by on Android: cpal's AAudio host and the system's `AudioDeviceInfo.getId()`.
const ID_PREFIX: &str = "aaudio:";

/// What an output is, to the picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputKind {
    /// The phone's earpiece.
    Earpiece,
    /// The phone's loudspeaker.
    Speaker,
    /// A built-in output the picker does not offer: a second loudspeaker path, the telephony path, the platform's bus.
    Hidden,
    /// Anything else (a headset, USB, Bluetooth): it keeps the name it gives itself. Also a kind this app does not know.
    #[serde(other)]
    External,
}

/// An output of the phone: the system's id of it, and its kind.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Output {
    pub id: i32,
    pub kind: OutputKind,
}

/// The phone's outputs by kind. `None` when the plugin does not answer (logged): the caller keeps what it knew. Empty where
/// the system names the outputs already.
pub fn outputs<R: Runtime>(app: &AppHandle<R>) -> Option<Vec<Output>> {
    platform::outputs(app)
}

/// The system's id of the voice's device `device_id`; `None` for what is not an AAudio device.
pub fn id_of(device_id: &str) -> Option<i32> {
    device_id.strip_prefix(ID_PREFIX)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_s_id_is_the_number_in_the_voice_s() {
        assert_eq!(id_of("aaudio:3"), Some(3));
        assert_eq!(id_of("aaudio:-1"), Some(-1));
        for id in ["alsa:3", "aaudio:", "aaudio:x", "3", ""] {
            assert_eq!(id_of(id), None, "{id}");
        }
    }

    #[test]
    fn the_answer_of_the_plugin_is_read_and_an_unknown_kind_is_external() {
        let answer: Vec<Output> = serde_json::from_str(
            r#"[{"id":2,"kind":"earpiece"},{"id":3,"kind":"speaker"},{"id":4,"kind":"hidden"},{"id":9,"kind":"external"},{"id":10,"kind":"novel"}]"#,
        )
        .unwrap();
        let kinds: Vec<OutputKind> = answer.iter().map(|output| output.kind).collect();
        assert_eq!(
            kinds,
            [
                OutputKind::Earpiece,
                OutputKind::Speaker,
                OutputKind::Hidden,
                OutputKind::External,
                OutputKind::External
            ]
        );
    }
}
