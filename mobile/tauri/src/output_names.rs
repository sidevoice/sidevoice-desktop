//! The outputs the page's picker lists, worded for the person: Android names the built-in ones by the phone's model
//! (`22101316G (Earpiece)`, `22101316G (Speaker)`, `22101316G`), so the plugin names them by type, in the person's language
//! (docs/MOBILE.md → "The outputs' names"). Every other device keeps the name it gives itself.
use sidevoice_mobile_plugin::{built_in_outputs, label_of, BuiltInOutput};
use sidevoice_shared::{app::DeviceLabels, sidevoice_voice::AudioDevices};
use tauri::AppHandle;

/// The devices' wording, asked of the Android plugin at every listing (the devices come and go).
pub struct BuiltInNames(pub AppHandle);

impl DeviceLabels for BuiltInNames {
    fn label(&self, devices: &mut AudioDevices) {
        rename(devices, &built_in_outputs(&self.0));
    }
}

/// The speakers among `devices` that are `built_in` outputs of the phone, named by type. The microphones and the rest stay.
fn rename(devices: &mut AudioDevices, built_in: &[BuiltInOutput]) {
    for output in &mut devices.outputs {
        if let Some(label) = label_of(built_in, &output.id) {
            output.label = label.to_owned();
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

    #[test]
    fn the_built_in_outputs_are_named_by_type_and_the_rest_keep_their_names() {
        let mut devices = AudioDevices {
            inputs: vec![device("aaudio:2", "22101316G (Microphone)")],
            outputs: vec![
                device("aaudio:-1", "Default Device"),
                device("aaudio:2", "22101316G (Earpiece)"),
                device("aaudio:3", "22101316G (Speaker)"),
                device("aaudio:4", "22101316G"),
                device("aaudio:9", "AirPods de Ana"),
            ],
            input: None,
            output: Some("aaudio:3".into()),
            output_selectable: true,
            lost: None,
        };
        let built_in = [
            BuiltInOutput {
                id: 2,
                label: "Auricular".into(),
            },
            BuiltInOutput {
                id: 3,
                label: "Altavoz".into(),
            },
            BuiltInOutput {
                id: 4,
                label: "Salida integrada".into(),
            },
        ];
        rename(&mut devices, &built_in);
        let labels: Vec<&str> = devices.outputs.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Default Device",
                "Auricular",
                "Altavoz",
                "Salida integrada",
                "AirPods de Ana"
            ]
        );
        assert_eq!(
            devices.inputs[0].label, "22101316G (Microphone)",
            "an input shares an id space with no output"
        );
        assert_eq!(
            devices.output.as_deref(),
            Some("aaudio:3"),
            "the choice is by id, which stays"
        );
    }

    #[test]
    fn without_an_answer_every_name_stays() {
        let mut devices = AudioDevices {
            inputs: vec![],
            outputs: vec![device("aaudio:3", "22101316G (Speaker)")],
            input: None,
            output: None,
            output_selectable: true,
            lost: None,
        };
        rename(&mut devices, &[]);
        assert_eq!(devices.outputs[0].label, "22101316G (Speaker)");
    }
}
