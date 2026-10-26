use super::Native;
use sidevoice_desktop_core::bridge::Capabilities;

pub(super) struct Os;

impl Native for Os {
    const CAPABILITIES: Capabilities = Capabilities { local_host: true, voice: true };
}
