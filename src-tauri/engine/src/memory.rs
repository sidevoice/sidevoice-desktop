//! This machine's memory, as its OS reports it: the total is what a catalogue model's `requires.memory_mb` is
//! weighed against when the page resolves its offers (sidevoice/sidevoice-core#21 §4); what is available now goes with
//! it to the page (`memory()`), which shows both and decides. `None` when the OS does not say.

/// Total physical memory in MiB.
pub fn total_mb() -> Option<u64> {
    total_bytes().map(|bytes| bytes / (1024 * 1024)).filter(|mb| *mb > 0)
}

/// Memory available to load something now, in MiB, as the OS judges it: Linux's `MemAvailable`, Windows'
/// `ullAvailPhys`, and on macOS the share of the total the kernel's memory-pressure level reports free
/// (`kern.memorystatus_level`, a percentage: macOS compresses and swaps rather than fail, so this is a gauge, not a
/// limit).
pub fn available_mb() -> Option<u64> {
    available_bytes().map(|bytes| bytes / (1024 * 1024))
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

#[cfg(target_os = "macos")]
fn available_bytes() -> Option<u64> {
    let mut level: u32 = 0;
    let mut size = std::mem::size_of::<u32>();
    // SAFETY: `kern.memorystatus_level` is a 32-bit integer; `level` and `size` outlive the call.
    let status = unsafe {
        libc::sysctlbyname(
            c"kern.memorystatus_level".as_ptr(),
            (&mut level as *mut u32).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 || level > 100 {
        return None;
    }
    total_bytes().map(|total| total / 100 * u64::from(level))
}

#[cfg(target_os = "linux")]
#[allow(clippy::unnecessary_cast)] // `c_ulong` is 64 bits here, 32 on other targets
fn total_bytes() -> Option<u64> {
    // SAFETY: `sysinfo` only writes into the zeroed struct it is given.
    let mut info: libc::sysinfo = unsafe { std::mem::zeroed() };
    let status = unsafe { libc::sysinfo(&mut info) };
    (status == 0).then(|| info.totalram as u64 * u64::from(info.mem_unit))
}

#[cfg(target_os = "linux")]
fn available_bytes() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = meminfo.lines().find_map(|l| l.strip_prefix("MemAvailable:"))?;
    let kib: u64 = line.trim().trim_end_matches("kB").trim().parse().ok()?;
    Some(kib * 1024)
}

#[cfg(windows)]
fn status() -> Option<windows_sys::Win32::System::SystemInformation::MEMORYSTATUSEX> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: a zeroed MEMORYSTATUSEX with its length set is what the call expects; it only writes into it.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    (ok != 0).then_some(status)
}

#[cfg(windows)]
fn total_bytes() -> Option<u64> {
    status().map(|s| s.ullTotalPhys)
}

#[cfg(windows)]
fn available_bytes() -> Option<u64> {
    status().map(|s| s.ullAvailPhys)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn total_bytes() -> Option<u64> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn available_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn this_machine_reports_its_memory_and_what_is_available_of_it() {
        let mb = super::total_mb().expect("the OS says how much memory there is");
        assert!(mb >= 256, "{mb} MiB");
        let available = super::available_mb().expect("the OS says how much is available");
        assert!(available <= mb, "{available} of {mb} MiB");
    }
}
