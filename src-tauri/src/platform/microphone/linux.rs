//! Linux has no microphone consent prompt and no settings URI: access is whether an input device exists.
use super::{input_devices, resolve, AccessState, MicrophoneAccess};

pub struct NativeMicrophone;
impl MicrophoneAccess for NativeMicrophone {
    fn state() -> Result<AccessState, &'static str> {
        Ok(resolve(AccessState::Granted, input_devices()))
    }
}
