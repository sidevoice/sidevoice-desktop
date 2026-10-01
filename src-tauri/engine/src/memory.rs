//! This machine's total memory, as its OS reports it: what a catalogue model's `requires.memory_mb` is weighed
//! against when the page resolves its offers (rubasace/sidevoice#124 §4). `None` when the OS does not say.

/// Total physical memory in MiB.
pub fn total_mb() -> Option<u64> {
    total_bytes().map(|bytes| bytes / (1024 * 1024)).filter(|mb| *mb > 0)
}

#[cfg(target_os = "macos")]
fn total_bytes() -> Option<u64> {
    let mut bytes: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    // SAFETY: `hw.memsize` is a 64-bit integer; `bytes` and `size` outlive the call and `size` is its length.
    let status = unsafe {
        libc::sysctlbyname(c"hw.memsize".as_ptr(), (&mut bytes as *mut u64).cast(), &mut size, std::ptr::null_mut(), 0)
    };
    (status == 0).then_some(bytes)
}

#[cfg(target_os = "linux")]
#[allow(clippy::unnecessary_cast)] // `c_ulong` is 64 bits here, 32 on other targets
fn total_bytes() -> Option<u64> {
    // SAFETY: `sysinfo` only writes into the zeroed struct it is given.
    let mut info: libc::sysinfo = unsafe { std::mem::zeroed() };
    let status = unsafe { libc::sysinfo(&mut info) };
    (status == 0).then(|| info.totalram as u64 * u64::from(info.mem_unit))
}

#[cfg(windows)]
fn total_bytes() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: a zeroed MEMORYSTATUSEX with its length set is what the call expects; it only writes into it.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    (ok != 0).then_some(status.ullTotalPhys)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn total_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn this_machine_reports_its_memory() {
        let mb = super::total_mb().expect("the OS says how much memory there is");
        assert!(mb >= 256, "{mb} MiB");
    }
}
