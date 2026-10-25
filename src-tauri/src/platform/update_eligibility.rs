//! Self-update eligibility; feed, download, install and restart remain owned by the updater plugin.
//! Tauri supplies bundle identity; Windows additionally verifies its NSIS installation record.

use sidevoice_desktop_core::updater;
use tauri::utils::{config::BundleType, platform::bundle_type};

#[cfg(target_os = "windows")]
#[path = "update_eligibility/windows.rs"]
mod imp;
#[cfg(not(target_os = "windows"))]
mod imp {
    pub struct Os;
    impl super::UpdateEligibility for Os {}
}
pub use imp::Os as NativeUpdateEligibility;

pub trait UpdateEligibility {
    /// A release/nightly running from a writable macOS app volume, an AppImage launched as such, or an NSIS
    /// installation can use the plugin's update/restart path. Dev builds, deb/rpm, MSI and bare binaries cannot.
    /// Windows also requires this executable to match the per-user NSIS registration.
    /// Bundle identity comes from Tauri; APPIMAGE is the AppImage runtime's own path, not a guessed executable suffix.
    fn available(channel: &str) -> bool {
        updater::feed(channel).is_some()
            && match bundle_type() {
                Some(BundleType::App) => std::env::current_exe().is_ok_and(|exe| {
                    updater::in_app_bundle(&exe) && exe.ancestors().nth(4).is_some_and(writable_volume)
                }),
                Some(BundleType::AppImage) => std::env::var_os("APPIMAGE").is_some_and(|path| !path.is_empty()),
                _ => false,
            }
    }
}

/// Bind a running executable to the actual NSIS record. NSIS quotes InstallLocation and records
/// MainBinaryName separately. Comparing both directory and file identity rejects copies/hard links
/// outside the installation and handles native path aliases/casing without hand-written normalization.
#[cfg(any(test, target_os = "windows"))]
fn installed_executable(exe: &std::path::Path, location: &str, binary: &str) -> bool {
    let location = location.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(location);
    let directory = std::path::Path::new(location);
    let mut components = std::path::Path::new(binary).components();
    if !directory.is_absolute()
        || !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return false;
    }
    exe.parent().is_some_and(|parent| same_file::is_same_file(parent, directory).unwrap_or(false))
        && same_file::is_same_file(exe, directory.join(binary)).unwrap_or(false)
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
    fn only_the_registered_executable_is_eligible_even_when_a_copy_has_identical_bytes() {
        let root = tempfile::tempdir().unwrap();
        let installed = root.path().join("Installed Sidevoice");
        let copied = root.path().join("Copied Sidevoice");
        std::fs::create_dir(&installed).unwrap();
        std::fs::create_dir(&copied).unwrap();
        let binary = "sidevoice-desktop.exe";
        let original = installed.join(binary);
        let copy = copied.join(binary);
        std::fs::write(&original, b"an NSIS-marked executable").unwrap();
        std::fs::copy(&original, &copy).unwrap();
        let location = format!("\"{}\"", installed.display());
        assert!(installed_executable(&original, &location, binary));
        assert!(!installed_executable(&copy, &location, binary));
        assert!(!installed_executable(&original, "", binary));
        assert!(!installed_executable(&original, "relative", binary));
        assert!(!installed_executable(&original, &location, "missing.exe"));
        assert!(!installed_executable(&original, &location, "../sidevoice-desktop.exe"));
    }

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
