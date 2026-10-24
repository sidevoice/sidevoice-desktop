use super::{TrayAppearance, TrayIcon, TrayImage};

pub struct Appearance;

impl TrayAppearance for Appearance {
    fn image(state: TrayIcon) -> TrayImage {
        let light = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
            .and_then(|key| key.get_value::<u32, _>("SystemUsesLightTheme"))
            .map(|light| light == 1)
            .unwrap_or(false);
        super::colored(state, light)
    }
}
