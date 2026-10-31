//! Linux has no microphone consent prompt and no settings URI: whether an input device exists is the shared rule's,
//! read from the sound server the page's audio goes through, never from ALSA hints (`pcm.null` is a hint with no
//! hardware behind it, and ALSA cannot see server-only sources).
//!
//! The PulseAudio host speaks the Pulse protocol in pure Rust (no libpulse, no build or runtime package) and PipeWire
//! serves that protocol through pipewire-pulse, so one host covers both servers. cpal's PipeWire host would add
//! libpipewire and libclang to the build and libpipewire to the runtime for no extra coverage. ALSA stays linked,
//! because cpal links it on Linux whatever the features, but it is never consulted.
use super::{AccessState, Os};
use cpal::{traits::DeviceTrait, HostId};

pub struct Native;
impl Os for Native {
    fn permission() -> Result<AccessState, &'static str> {
        Ok(AccessState::Granted)
    }

    fn audio_host() -> Option<cpal::Host> {
        // `default_host` would fall back to ALSA when the server is down.
        cpal::host_from_id(HostId::PulseAudio).ok()
    }

    fn is_capture_source(device: &cpal::Device) -> bool {
        // Pulse and pipewire-pulse list every output's monitor as an input, always named "<sink>.monitor". cpal does
        // not say which sources are monitors; a monitor is not a microphone.
        device.id().map_or(true, |id| !id.id().ends_with(".monitor"))
    }
}
