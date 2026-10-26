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
