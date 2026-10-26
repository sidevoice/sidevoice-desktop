//! What the host offers the page beyond the web: this computer's own core (the local host) and the voice call run by
//! the app. The bridge only tells the page; the callers that gate on a capability ask here too.
//!
//! macOS offers both (the beta's app platform, design O2, where the echo canceller builds); elsewhere the app is a
//! remote client and the page runs the call. Only this value differs by OS.

use sidevoice_desktop_core::bridge::Capabilities;

#[cfg_attr(target_os = "macos", path = "host_capabilities/macos.rs")]
#[cfg_attr(target_os = "windows", path = "host_capabilities/windows.rs")]
#[cfg_attr(target_os = "linux", path = "host_capabilities/linux.rs")]
mod imp;
use imp::Native;

/// What differs per OS: what it offers.
trait Os {
    /// Nothing beyond the web unless the OS offers it.
    const CAPABILITIES: Capabilities = Capabilities { local_host: false, voice: false };
}

/// What this host offers the page beyond the web.
pub fn capabilities() -> Capabilities {
    Native::CAPABILITIES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_offers_nothing_beyond_the_web_unless_it_says_so() {
        struct Remote;
        impl Os for Remote {}
        assert_eq!(Remote::CAPABILITIES, Capabilities { local_host: false, voice: false });
    }
}
