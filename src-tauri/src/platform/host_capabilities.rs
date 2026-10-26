//! What the host offers the page beyond the web: this computer's own core (the local host) and the voice call run by
//! the app. The bridge only tells the page; the callers that gate on a capability ask here too.
//!
//! macOS offers both (the beta's app platform, design O2, where the echo canceller builds); elsewhere the app is a
//! remote client and the page runs the call.

use sidevoice_desktop_core::bridge::Capabilities;

pub trait HostCapabilities {
    /// What this host offers; none by default.
    fn capabilities() -> Capabilities {
        Capabilities::default()
    }
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::NativeHostCapabilities;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::NativeHostCapabilities;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::NativeHostCapabilities;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_that_says_nothing_offers_nothing() {
        struct Silent;
        impl HostCapabilities for Silent {}
        assert_eq!(Silent::capabilities(), Capabilities { local_host: false, voice: false });
    }

    #[test]
    fn only_macos_offers_the_local_host_and_the_voice_call() {
        let offered = cfg!(target_os = "macos");
        assert_eq!(NativeHostCapabilities::capabilities(), Capabilities { local_host: offered, voice: offered });
    }
}
