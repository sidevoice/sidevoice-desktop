//! Linux has no independent TCC-style prompt: WebKit's existing capture backend supplies the outcome.
//! No second PipeWire/Pulse connection or portal.
use super::MicrophoneAccess;

pub struct NativeMicrophone;
impl MicrophoneAccess for NativeMicrophone {}
