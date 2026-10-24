//! The name the person gave this computer (`host.computerName`, docs/BRIDGE.md), not an account or network name.

#[cfg(target_os = "macos")]
#[path = "computer_name/macos.rs"]
mod imp;
#[cfg(target_os = "windows")]
#[path = "computer_name/windows.rs"]
mod imp;
#[cfg(target_os = "linux")]
#[path = "computer_name/linux.rs"]
mod imp;

pub use imp::Name as SystemComputerName;

pub trait ComputerName {
    /// Read on every room open so a rename appears on the next one. macOS uses Sharing's Computer Name,
    /// Windows the physical DNS device name (preserving case), and Linux the pretty hostname then the kernel
    /// hostname. Trim surrounding whitespace; return None for absent/blank names so the page can name the device.
    fn read() -> Option<String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_system_name_is_nonblank_and_preserves_its_given_spelling() {
        if let Some(name) = SystemComputerName::read() {
            assert!(!name.is_empty());
            assert_eq!(name, name.trim());
            assert_eq!(sidevoice_desktop_core::computer_name::given(&name).as_deref(), Some(name.as_str()));
        }
    }
}
