use super::{Memory, MemorySnapshot};

pub struct Os;

impl Memory for Os {
    fn snapshot() -> MemorySnapshot {
        MemorySnapshot { total_bytes: total_bytes(), available_bytes: available_bytes() }
    }
}

fn total_bytes() -> Option<u64> {
    let mut bytes: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    // SAFETY: `hw.memsize` is a 64-bit integer; `bytes` and `size` outlive the call and `size` is its length.
    let status = unsafe {
        libc::sysctlbyname(c"hw.memsize".as_ptr(), (&mut bytes as *mut u64).cast(), &mut size, std::ptr::null_mut(), 0)
    };
    (status == 0).then_some(bytes)
}

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
