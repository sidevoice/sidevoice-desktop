use super::UpdateEligibility;
use sidevoice_desktop_core::updater;
use tauri::utils::{config::BundleType, platform::bundle_type};

pub struct NativeUpdateEligibility;

impl UpdateEligibility for NativeUpdateEligibility {
    /// Only an AppImage can update itself; deb/rpm are managed by the system. APPIMAGE is the AppImage runtime's
    /// own path, not a guessed executable suffix.
    fn available(channel: &str) -> bool {
        updater::feed(channel).is_some()
            && bundle_type() == Some(BundleType::AppImage)
            && std::env::var_os("APPIMAGE").is_some_and(|path| !path.is_empty())
    }
}
