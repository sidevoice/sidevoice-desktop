//! What the host computer is and offers: its name, what the page may use beyond the web, whether reset exists, and
//! whether this run can update itself. Per-OS differences live in `host/{macos,windows,linux}.rs`.

use sidevoice_desktop_core::bridge::Capabilities;
use sidevoice_desktop_core::{computer_name, updater};
use tauri::utils::{config::BundleType, platform::bundle_type};

#[cfg_attr(target_os = "macos", path = "host/macos.rs")]
#[cfg_attr(target_os = "windows", path = "host/windows.rs")]
#[cfg_attr(target_os = "linux", path = "host/linux.rs")]
mod imp;
use imp::Native;

/// Whether Desktop offers reset (delete all data, uninstall) on this host. When false, callers refuse before any
/// confirmation or data access, and a reset helper only completes the committed wait: the handoff
/// (`reset_handoff`) works on every host either way.
///
/// Only the reset cleanup itself (data folders, WebKit data, credentials, app removal) differs: it exists for macOS
/// only and still lives in `data.rs`. Only this value differs by OS, so there is no per-OS code. It moves to the reset
/// cleanup concern when that exists (plan rows 15, 16, 42–44) and disappears once Windows and Linux get their cleanup.
pub const RESET_AVAILABLE: bool = cfg!(target_os = "macos");

/// The origin Tauri serves the app's own pages from: what a node sees as the interface's `Origin`. Only this value
/// differs by OS, so there is no per-OS code.
pub const APP_ORIGIN: &str = if cfg!(windows) { "http://tauri.localhost" } else { "tauri://localhost" };

/// Whether this host is an Apple-silicon Mac, where the recommended Whisper build is the Metal one. Read only by the
/// real-models call test.
#[cfg(test)]
pub const APPLE_SILICON: bool = cfg!(all(target_os = "macos", target_arch = "aarch64"));

/// What differs per OS.
trait Os {
    /// The name the person gave the device, kept apart from the network name (macOS Sharing's Computer Name,
    /// Linux's pretty hostname); `None` where the OS keeps none.
    fn device_name() -> Option<String> {
        None
    }

    /// The host name, read when there is no device name (Windows' physical DNS name preserving case, Linux's
    /// kernel hostname).
    fn host_name() -> Option<String> {
        None
    }

    /// What the host offers the page beyond the web: this computer's own core (the local host) and the voice call
    /// run by the app. Nothing beyond the web unless the OS offers it; macOS offers both (the beta's app platform,
    /// design O2, where the echo canceller builds), elsewhere the app is a remote client and the page runs the call.
    const CAPABILITIES: Capabilities = Capabilities { local_host: false, voice: false };

    /// The bundle the updater plugin can update and restart here; `None` where there is none.
    const BUNDLE: Option<BundleType> = None;

    /// Whether this run is that bundle's installation, not a copy of it.
    fn installed_here() -> bool {
        false
    }
}

/// The name the person gave this computer (`host.computerName`, docs/BRIDGE.md), not an account or network name.
/// Read on every room open so a rename appears on the next one: the device name, else the host name. Trim
/// surrounding whitespace; return None for absent/blank names so the page can name the device.
pub fn name() -> Option<String> {
    Native::device_name().or_else(Native::host_name).and_then(|name| computer_name::given(&name))
}

/// What this host offers the page beyond the web. The bridge only tells the page; the callers that gate on a
/// capability ask here too.
pub fn capabilities() -> Capabilities {
    Native::CAPABILITIES
}

/// Whether this run is an installation the updater plugin can update and restart: a release/nightly running
/// from a writable macOS app volume, an AppImage launched as such, or the registered per-user NSIS installation.
/// Dev builds, deb/rpm, MSI, bare binaries and hosts without an update path report `false`.
/// Feed, download, install and restart remain owned by the updater plugin; Tauri's bundle identity decides on every
/// OS, and each OS adds its own proof that this run is that installation.
pub fn updates_available(channel: &str) -> bool {
    updater::feed(channel).is_some()
        && Native::BUNDLE.is_some_and(|bundle| bundle_type() == Some(bundle))
        && Native::installed_here()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_system_name_is_nonblank_and_preserves_its_given_spelling() {
        if let Some(name) = name() {
            assert!(!name.is_empty());
            assert_eq!(name, name.trim());
            assert_eq!(computer_name::given(&name).as_deref(), Some(name.as_str()));
        }
    }

    #[test]
    fn a_host_offers_nothing_beyond_the_web_unless_it_says_so() {
        struct Remote;
        impl Os for Remote {}
        assert_eq!(Remote::CAPABILITIES, Capabilities { local_host: false, voice: false });
    }

    #[test]
    fn dev_and_unknown_channels_never_offer_updates() {
        for channel in ["dev", "", "other"] {
            assert!(!updates_available(channel));
        }
    }

    #[test]
    fn release_and_nightly_share_the_installation_requirement() {
        assert_eq!(updates_available("release"), updates_available("nightly"));
        // These tests run as an ordinary test executable, outside any supported updater bundle.
        assert!(!updates_available("release"));
    }
}
