use super::HostCapabilities;
use sidevoice_desktop_core::bridge::Capabilities;

pub struct NativeHostCapabilities;

impl HostCapabilities for NativeHostCapabilities {
    fn capabilities() -> Capabilities {
        Capabilities { local_host: true, voice: true }
    }
}
