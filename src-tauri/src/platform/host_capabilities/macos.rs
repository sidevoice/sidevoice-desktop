use super::Os;
use sidevoice_desktop_core::bridge::Capabilities;

pub(super) struct Native;

impl Os for Native {
    const CAPABILITIES: Capabilities = Capabilities { local_host: true, voice: true };
}
