use sidevoice_desktop_core::computer_name;

pub struct Name;

impl super::ComputerName for Name {
    fn read() -> Option<String> {
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
        computer_name::given(&String::from_utf16_lossy(&buffer[..size as usize]))
    }
}
