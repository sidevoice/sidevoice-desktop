use super::Native;

pub(super) struct Os;

impl Native for Os {
    fn light_panel() -> Option<bool> {
        winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
            .and_then(|key| key.get_value::<u32, _>("SystemUsesLightTheme"))
            .map(|light| light == 1)
            .ok()
    }
}
