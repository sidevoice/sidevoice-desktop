//! The outputs the page's picker lists, worded for the person: Android names the built-in ones by the phone's model
//! (`22101316G (Earpiece)`, `22101316G (Speaker)`, `22101316G`), so the plugin tells each by its type and the app words the
//! kind in the page's language (`callCard.language`, the last the page reported), from the same tables as the page's other
//! texts (`devices.output.*` in `i18n/android/<language>.json`). A built-in output the picker does not offer is left out.
//! Every other device keeps the name it gives itself (docs/MOBILE.md → "The outputs' names").
use std::{collections::HashMap, sync::Mutex};

use sidevoice_mobile_plugin::{id_of, outputs, OutputKind};
use sidevoice_shared::{app::DeviceLabels, sidevoice_voice::AudioDevices};
use tauri::{AppHandle, Manager};

const EARPIECE: &str = "devices.output.earpiece";
const SPEAKER: &str = "devices.output.speaker";

/// The devices' wording. An output is asked of the plugin once, the first time it is listed (the devices come and go, but
/// an id is one device): the listing every two seconds does not reach the plugin, and a plugin that does not answer leaves
/// what was known.
pub struct OutputNames {
    app: AppHandle,
    words: serde_json::Value,
    kinds: Mutex<HashMap<i32, OutputKind>>,
}

impl OutputNames {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            words: crate::bridge::messages(),
            kinds: Mutex::default(),
        }
    }

    /// The page's language, as it last said, English before it has.
    fn language(&self) -> String {
        let last = self.app.state::<crate::LastReport>();
        let report = last.0.lock().unwrap_or_else(|e| e.into_inner());
        report
            .language
            .as_str()
            .filter(|language| !language.is_empty())
            .unwrap_or("en")
            .to_owned()
    }

    fn word(&self, language: &str, key: &str) -> Option<&str> {
        [language, "en"]
            .iter()
            .find_map(|language| self.words[*language][key].as_str())
    }
}

impl DeviceLabels for OutputNames {
    fn label(&self, devices: &mut AudioDevices) {
        let mut kinds = self.kinds.lock().unwrap_or_else(|e| e.into_inner());
        let unknown = devices
            .outputs
            .iter()
            .filter_map(|device| id_of(&device.id))
            .any(|id| id >= 0 && !kinds.contains_key(&id));
        if unknown {
            for output in outputs(&self.app).unwrap_or_default() {
                kinds.insert(output.id, output.kind);
            }
        }
        let language = self.language();
        rename(
            devices,
            &kinds,
            self.word(&language, EARPIECE),
            self.word(&language, SPEAKER),
        );
    }
}

/// The speakers among `devices` by their `kinds`: the earpiece and the loudspeaker named by `earpiece` and `speaker` (kept as
/// the system names them when the app has no word), the hidden ones left out. The microphones and the rest stay.
fn rename(
    devices: &mut AudioDevices,
    kinds: &HashMap<i32, OutputKind>,
    earpiece: Option<&str>,
    speaker: Option<&str>,
) {
    let kind = |id: &str| id_of(id).and_then(|id| kinds.get(&id).copied());
    devices
        .outputs
        .retain(|device| kind(&device.id) != Some(OutputKind::Hidden));
    for output in &mut devices.outputs {
        let word = match kind(&output.id) {
            Some(OutputKind::Earpiece) => earpiece,
            Some(OutputKind::Speaker) => speaker,
            _ => None,
        };
        if let Some(word) = word {
            output.label = word.to_owned();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidevoice_shared::sidevoice_voice::AudioDevice;

    fn device(id: &str, label: &str) -> AudioDevice {
        AudioDevice {
            id: id.into(),
            label: label.into(),
        }
    }

    fn listing() -> AudioDevices {
        AudioDevices {
            inputs: vec![device("aaudio:12", "22101316G (Microphone)")],
            outputs: vec![
                device("aaudio:-1", "Default Device"),
                device("aaudio:2", "22101316G (Earpiece)"),
                device("aaudio:3", "22101316G (Speaker)"),
                device("aaudio:4", "22101316G (Speaker)"),
                device("aaudio:5", "22101316G"),
                device("aaudio:9", "AirPods de Ana"),
            ],
            input: None,
            output: Some("aaudio:3".into()),
            output_selectable: true,
            lost: None,
        }
    }

    fn kinds() -> HashMap<i32, OutputKind> {
        HashMap::from([
            (2, OutputKind::Earpiece),
            (3, OutputKind::Speaker),
            (4, OutputKind::Hidden),
            (5, OutputKind::Hidden),
            (9, OutputKind::External),
        ])
    }

    #[test]
    fn the_earpiece_and_the_loudspeaker_are_named_by_type_the_hidden_are_left_out_and_the_rest_keep_their_names(
    ) {
        let mut devices = listing();
        rename(&mut devices, &kinds(), Some("Teléfono"), Some("Altavoz"));
        let labels: Vec<&str> = devices.outputs.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Default Device", "Teléfono", "Altavoz", "AirPods de Ana"]
        );
        assert_eq!(
            devices.inputs[0].label, "22101316G (Microphone)",
            "an input is left alone"
        );
        assert_eq!(
            devices.output.as_deref(),
            Some("aaudio:3"),
            "the choice is by id, which stays"
        );
    }

    #[test]
    fn no_two_outputs_are_given_the_same_name() {
        let mut devices = listing();
        rename(&mut devices, &kinds(), Some("Teléfono"), Some("Altavoz"));
        let mut labels: Vec<&str> = devices.outputs.iter().map(|d| d.label.as_str()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), devices.outputs.len());
    }

    #[test]
    fn without_a_word_or_a_kind_the_system_s_name_stays() {
        let mut devices = listing();
        rename(&mut devices, &HashMap::new(), None, None);
        assert_eq!(devices, listing());
        rename(&mut devices, &kinds(), None, None);
        assert_eq!(devices.outputs[1].label, "22101316G (Earpiece)");
    }
}
