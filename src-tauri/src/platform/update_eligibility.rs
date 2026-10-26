//! Self-update eligibility; feed, download, install and restart remain owned by the updater plugin.
//! Tauri's bundle identity decides on every OS; each OS adds its own proof that this run is that installation.

use sidevoice_desktop_core::updater;
use tauri::utils::{config::BundleType, platform::bundle_type};

#[cfg_attr(target_os = "macos", path = "update_eligibility/macos.rs")]
#[cfg_attr(target_os = "windows", path = "update_eligibility/windows.rs")]
#[cfg_attr(target_os = "linux", path = "update_eligibility/linux.rs")]
mod imp;
use imp::Os;

/// What differs per OS: which bundle the updater can replace, and how this run proves it is that installation.
trait Native {
    /// The bundle the updater plugin can update and restart here; `None` where there is none.
    const BUNDLE: Option<BundleType> = None;

    /// Whether this run is that bundle's installation, not a copy of it.
    fn installed_here() -> bool {
        false
    }
}

/// Whether this run is an installation the updater plugin can update and restart: a release/nightly running
/// from a writable macOS app volume, an AppImage launched as such, or the registered per-user NSIS installation.
/// Dev builds, deb/rpm, MSI, bare binaries and hosts without an update path report `false`.
pub fn available(channel: &str) -> bool {
    updater::feed(channel).is_some()
        && Os::BUNDLE.is_some_and(|bundle| bundle_type() == Some(bundle))
        && Os::installed_here()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_and_unknown_channels_never_offer_updates() {
        for channel in ["dev", "", "other"] {
            assert!(!available(channel));
        }
    }

    #[test]
    fn release_and_nightly_share_the_installation_requirement() {
        assert_eq!(available("release"), available("nightly"));
        // These tests run as an ordinary test executable, outside any supported updater bundle.
        assert!(!available("release"));
    }
}
