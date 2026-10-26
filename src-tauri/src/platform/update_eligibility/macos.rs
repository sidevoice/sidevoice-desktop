use super::UpdateEligibility;
use sidevoice_desktop_core::updater;
use tauri::utils::{config::BundleType, platform::bundle_type};

pub struct NativeUpdateEligibility;

impl UpdateEligibility for NativeUpdateEligibility {
    /// Only a release/nightly running from a writable app volume can use the plugin's update/restart path.
    fn available(channel: &str) -> bool {
        updater::feed(channel).is_some()
            && bundle_type() == Some(BundleType::App)
            && std::env::current_exe()
                .is_ok_and(|exe| updater::in_app_bundle(&exe) && exe.ancestors().nth(4).is_some_and(writable_volume))
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
