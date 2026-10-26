//! Self-update eligibility; feed, download, install and restart remain owned by the updater plugin.
//! Each OS file decides from Tauri's bundle identity; Windows additionally verifies its NSIS installation record.

pub trait UpdateEligibility {
    /// Whether this run is an installation the updater plugin can update and restart: a release/nightly running
    /// from a writable macOS app volume, an AppImage launched as such, or the registered per-user NSIS installation.
    /// Dev builds, deb/rpm, MSI, bare binaries and hosts without an update path report `false`.
    fn available(_channel: &str) -> bool {
        false
    }
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::NativeUpdateEligibility;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::NativeUpdateEligibility;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::NativeUpdateEligibility;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_and_unknown_channels_never_offer_updates() {
        for channel in ["dev", "", "other"] {
            assert!(!NativeUpdateEligibility::available(channel));
        }
    }

    #[test]
    fn release_and_nightly_share_the_installation_requirement() {
        assert_eq!(NativeUpdateEligibility::available("release"), NativeUpdateEligibility::available("nightly"));
        // These tests run as an ordinary test executable, outside any supported updater bundle.
        assert!(!NativeUpdateEligibility::available("release"));
    }
}
