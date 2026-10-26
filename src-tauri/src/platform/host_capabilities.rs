//! What the host offers the page beyond the web: this computer's own core (the local host) and the voice call run by
//! the app. The bridge only tells the page; the callers that gate on a capability ask here too.
//!
//! macOS offers both (the beta's app platform, design O2, where the echo canceller builds); elsewhere the app is a
//! remote client and the page runs the call. Only this value differs by OS, so there is no per-OS code.

use sidevoice_desktop_core::bridge::Capabilities;

#[cfg(target_os = "macos")]
pub const CAPABILITIES: Capabilities = Capabilities { local_host: true, voice: true };
#[cfg(not(target_os = "macos"))]
pub const CAPABILITIES: Capabilities = Capabilities { local_host: false, voice: false };

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_macos_offers_the_local_host_and_the_voice_call() {
        let offered = cfg!(target_os = "macos");
        assert_eq!(CAPABILITIES, Capabilities { local_host: offered, voice: offered });
    }
}
