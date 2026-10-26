use super::Os;
use tauri::utils::config::BundleType;

pub(super) struct Native;

impl Os for Native {
    /// Only an AppImage can update itself; deb/rpm are managed by the system.
    const BUNDLE: Option<BundleType> = Some(BundleType::AppImage);

    /// APPIMAGE is the AppImage runtime's own path, not a guessed executable suffix.
    fn installed_here() -> bool {
        std::env::var_os("APPIMAGE").is_some_and(|path| !path.is_empty())
    }
}
