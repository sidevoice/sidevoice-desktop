//! iOS: `AVAudioSession.availableInputs` and `setPreferredInput`. The ports' ids are the system's own (`UID`), their names
//! the system's (`portName`: "iPhone Microphone", the car's, the AirPods'). The session's category, mode and options are
//! the call's (`call::CALL_AUDIO`); nothing here touches them. The session answers from any thread.
use objc2_avf_audio::AVAudioSession;
use sidevoice_shared::app::SystemInputs;
use sidevoice_shared::sidevoice_voice::AudioDevice;

use super::Os;

pub struct Native;

impl Os for Native {
    fn inputs() -> Option<SystemInputs> {
        // SAFETY: messages to the shared session, read-only.
        let (ports, preferred) = unsafe {
            let session = AVAudioSession::sharedInstance();
            (session.availableInputs()?, session.preferredInput())
        };
        let inputs = ports
            .iter()
            // SAFETY: reads of live port descriptions.
            .map(|port| unsafe {
                AudioDevice {
                    id: port.UID().to_string(),
                    label: port.portName().to_string(),
                }
            })
            .collect();
        // SAFETY: as above.
        let chosen = preferred.map(|port| unsafe { port.UID().to_string() });
        Some(SystemInputs { inputs, chosen })
    }

    fn prefer(id: Option<&str>) -> Result<(), String> {
        // SAFETY: messages to the shared session; the port is one it listed a moment ago.
        unsafe {
            let session = AVAudioSession::sharedInstance();
            let port = match id {
                None => None,
                Some(id) => Some(
                    session
                        .availableInputs()
                        .and_then(|ports| ports.iter().find(|port| port.UID().to_string() == id))
                        .ok_or_else(|| "audio-device-unknown".to_owned())?,
                ),
            };
            session
                .setPreferredInput_error(port.as_deref())
                .map_err(|error| {
                    log::warn!(
                        "the audio session refused the preferred input: {} ({})",
                        error.localizedDescription(),
                        error.code()
                    );
                    "audio-input-failed".to_owned()
                })
        }
    }
}
