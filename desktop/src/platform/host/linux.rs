use super::Os;
use sidevoice_desktop_core::computer_name;
use tauri::utils::config::BundleType;

pub(super) struct Native;

impl Os for Native {
    const SHORTCUT_MODIFIER: &'static str = "Ctrl";
    const MICROPHONE_HELP: &'static str = "microphone.help_linux";
    const MICROPHONE_SETTINGS_LABEL: Option<&'static str> = None;

    fn device_name() -> Option<String> {
        std::fs::read_to_string("/etc/machine-info").ok().and_then(|file| computer_name::pretty_hostname(&file))
    }

    fn host_name() -> Option<String> {
        whoami::hostname().ok()
    }

    /// Only an AppImage can update itself; deb/rpm are managed by the system.
    const BUNDLE: Option<BundleType> = Some(BundleType::AppImage);

    /// APPIMAGE is the AppImage runtime's own path, not a guessed executable suffix.
    fn installed_here() -> bool {
        std::env::var_os("APPIMAGE").is_some_and(|path| !path.is_empty())
    }
}
