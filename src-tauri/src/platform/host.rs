//! What the host computer is and offers: its name, whether reset exists, and whether this run can update itself. Per-OS differences live in `host/{macos,windows,linux}.rs`.

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
/// The deletion itself (`app_data`, with `webview_data` for browser storage) has a body on every host; what is still
/// macOS-only is the rest of the transaction in `data.rs` (connector service, keychain; plan rows 15, 16, 42–44).
/// Only this value differs by OS, so there is no per-OS code. It disappears once Windows and Linux get those steps.
pub const RESET_AVAILABLE: bool = cfg!(target_os = "macos");

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
