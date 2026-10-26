//! The name the person gave this computer (`host.computerName`, docs/BRIDGE.md), not an account or network name.

use sidevoice_desktop_core::computer_name;

#[cfg_attr(target_os = "macos", path = "computer_name/macos.rs")]
#[cfg_attr(target_os = "windows", path = "computer_name/windows.rs")]
#[cfg_attr(target_os = "linux", path = "computer_name/linux.rs")]
mod imp;
use imp::Native;

/// Where each OS keeps the name; `None` where it keeps none.
trait Os {
    /// The name the person gave the device, kept apart from the network name (macOS Sharing's Computer Name,
    /// Linux's pretty hostname).
    fn device_name() -> Option<String> {
        None
    }

    /// The host name, read when there is no device name (Windows' physical DNS name preserving case, Linux's
    /// kernel hostname).
    fn host_name() -> Option<String> {
        None
    }
}

/// Read on every room open so a rename appears on the next one: the device name, else the host name. Trim
/// surrounding whitespace; return None for absent/blank names so the page can name the device.
pub fn read() -> Option<String> {
    Native::device_name().or_else(Native::host_name).and_then(|name| computer_name::given(&name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_system_name_is_nonblank_and_preserves_its_given_spelling() {
        if let Some(name) = read() {
            assert!(!name.is_empty());
            assert_eq!(name, name.trim());
            assert_eq!(computer_name::given(&name).as_deref(), Some(name.as_str()));
        }
    }
}
