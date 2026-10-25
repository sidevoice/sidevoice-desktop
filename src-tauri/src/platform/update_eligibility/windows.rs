use super::{installed_executable, UpdateEligibility};
use sidevoice_desktop_core::updater;
use tauri::utils::{config::BundleType, platform::bundle_type};
use winreg::{enums::HKEY_CURRENT_USER, RegKey};

pub struct Os;

impl UpdateEligibility for Os {
    fn available(channel: &str) -> bool {
        if updater::feed(channel).is_none() || bundle_type() != Some(BundleType::Nsis) {
            return false;
        }
        registration().is_some_and(|(location, binary)| {
            std::env::current_exe().is_ok_and(|exe| installed_executable(&exe, &location, &binary))
        })
    }
}

fn registration() -> Option<(String, String)> {
    // Tauri's NSIS template keys the uninstall entry by PRODUCTNAME, not BUNDLEID.
    let config: serde_json::Value = serde_json::from_str(include_str!("../../../tauri.conf.json")).ok()?;
    let product = config.get("productName")?.as_str()?;
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{product}"))
        .ok()?;
    Some((key.get_value("InstallLocation").ok()?, key.get_value("MainBinaryName").ok()?))
}
