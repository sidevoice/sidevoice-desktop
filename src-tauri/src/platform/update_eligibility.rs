//! Self-update eligibility; feed, download, install and restart remain owned by the updater plugin.
//! Tauri supplies the bundle identity on every OS, so this concern needs one library-backed body.

use sidevoice_desktop_core::updater;
use tauri::utils::{config::BundleType, platform::bundle_type};

mod imp {
    pub struct Os;
    impl super::UpdateEligibility for Os {}
}
pub use imp::Os as NativeUpdateEligibility;

pub trait UpdateEligibility {
    /// A release/nightly running from a writable macOS app volume, an AppImage launched as such, or an NSIS
    /// installation can use the plugin's update/restart path. Dev builds, deb/rpm, MSI and bare binaries cannot.
    /// Bundle identity comes from Tauri; APPIMAGE is the AppImage runtime's own path, not a guessed executable suffix.
    fn available(channel: &str) -> bool {
        updater::feed(channel).is_some()
            && match bundle_type() {
                Some(BundleType::App) => std::env::current_exe().is_ok_and(|exe| {
                    updater::in_app_bundle(&exe) && exe.ancestors().nth(4).is_some_and(writable_volume)
                }),
                Some(BundleType::AppImage) => std::env::var_os("APPIMAGE").is_some_and(|path| !path.is_empty()),
                Some(BundleType::Nsis) => true,
                _ => false,
            }
    }
}

/// Preserve the volume check: failure for reasons other than a read-only filesystem is left to the updater.
/// Only unlink a probe we created, so an existing file is never removed during eligibility discovery.
fn writable_volume(folder: &std::path::Path) -> bool {
    let probe = folder.join(format!(".sidevoice-update-check-{}", std::process::id()));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(file) => {
            drop(file);
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(error) => error.kind() != std::io::ErrorKind::ReadOnlyFilesystem,
    }
}

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
