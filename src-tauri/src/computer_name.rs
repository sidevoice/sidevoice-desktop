//! `host.computerName` (docs/BRIDGE.md): the name the person gave this computer, asked of the system each time the
//! room window opens (a rename shows at the next one). None when the system has none: the page then names the device
//! itself.

use sidevoice_desktop_core::computer_name;

pub fn read() -> Option<String> {
    system().and_then(|name| computer_name::given(&name))
}

/// macOS: the Computer Name (System Settings → General → Sharing, "Mac de Ana"), not the `.local` hostname.
#[cfg(target_os = "macos")]
fn system() -> Option<String> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    use system_configuration_sys::dynamic_store_copy_specific::SCDynamicStoreCopyComputerName;
    // No store (a temporary session) and no encoding wanted; the name follows the Create Rule: ours to release.
    let name = unsafe { SCDynamicStoreCopyComputerName(std::ptr::null(), std::ptr::null_mut()) };
    if name.is_null() {
        return None;
    }
    Some(unsafe { CFString::wrap_under_create_rule(name) }.to_string())
}

/// Windows: the device name (Settings → System → About), as typed: the DNS host name, not the upper-cased NetBIOS one.
#[cfg(windows)]
fn system() -> Option<String> {
    use windows_sys::Win32::System::SystemInformation::{ComputerNamePhysicalDnsHostname, GetComputerNameExW};
    // The first call fails and says the size the name needs, its terminating nul included.
    let mut size = 0u32;
    unsafe { GetComputerNameExW(ComputerNamePhysicalDnsHostname, std::ptr::null_mut(), &mut size) };
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0u16; size as usize];
    // Then `size` is the name's length, without the nul.
    if unsafe { GetComputerNameExW(ComputerNamePhysicalDnsHostname, buffer.as_mut_ptr(), &mut size) } == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..size as usize]))
}

/// Linux: the pretty hostname if the person set one (`hostnamectl set-hostname --pretty`), else the hostname.
#[cfg(all(unix, not(target_os = "macos")))]
fn system() -> Option<String> {
    let pretty =
        std::fs::read_to_string("/etc/machine-info").ok().and_then(|file| computer_name::pretty_hostname(&file));
    pretty.or_else(|| std::fs::read_to_string("/proc/sys/kernel/hostname").ok())
}
