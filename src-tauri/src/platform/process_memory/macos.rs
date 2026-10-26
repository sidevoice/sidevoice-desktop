use super::ProcessMemory;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use sidevoice_desktop_core::memory::{accelerator_bytes, Sample};

pub struct SystemProcessMemory;

#[link(name = "Metal", kind = "framework")]
extern "C" {
    /// The system's default GPU; null where there is none. Owned by the caller (a "create" function).
    fn MTLCreateSystemDefaultDevice() -> *mut AnyObject;
}

#[allow(non_upper_case_globals)]
extern "C" {
    static mach_task_self_: u32;
    fn task_info(task: u32, flavor: u32, info: *mut u32, count: *mut u32) -> i32;
}

/// `TASK_VM_INFO`, and where `phys_footprint` and `resident_size` lie in `task_vm_info_data_t` (a `mach_vm_size_t`
/// is 8 bytes: `virtual_size`, then two `int`s, then `resident_size` at 16 ... `phys_footprint` at 144).
const TASK_VM_INFO: u32 = 22;
const RESIDENT_AT: usize = 16;
const FOOTPRINT_AT: usize = 144;

/// The footprint and the resident set, in bytes.
fn task() -> (Option<u64>, Option<u64>) {
    // The kernel fills as many 32-bit words as it has (the struct grows between releases); room for more than ever.
    let mut info = [0u32; 128];
    let mut count = info.len() as u32;
    // SAFETY: `info` has `count` words; `mach_task_self_` is this process's task port, set before main.
    let status = unsafe { task_info(mach_task_self_, TASK_VM_INFO, info.as_mut_ptr(), &mut count) };
    if status != 0 || (count as usize) * 4 < FOOTPRINT_AT + 8 {
        return (None, None);
    }
    let word = |at: usize| u64::from(info[at / 4]) | (u64::from(info[at / 4 + 1]) << 32);
    (Some(word(FOOTPRINT_AT)), Some(word(RESIDENT_AT)))
}

/// What the process holds on the GPU through Metal: `currentAllocatedSize` of the default device. `None` with no GPU.
fn metal() -> Option<u64> {
    // SAFETY: a plain C function; the device it returns is released below, once.
    let device = unsafe { Retained::from_raw(MTLCreateSystemDefaultDevice()) }?;
    // SAFETY: an MTLDevice answers `currentAllocatedSize` with an NSUInteger.
    let size: usize = unsafe { msg_send![&*device, currentAllocatedSize] };
    Some(size as u64)
}

/// The GPU's mapped regions, read by the system's own `vmmap` on this process (it can take a second or two).
fn accelerator() -> Option<u64> {
    let out = std::process::Command::new("/usr/bin/vmmap")
        .args(["-summary", &std::process::id().to_string()])
        .output()
        .ok()?;
    accelerator_bytes(&String::from_utf8_lossy(&out.stdout))
}

/// A look at the process now.
impl ProcessMemory for SystemProcessMemory {
    fn sample() -> Sample {
        let (footprint, resident) = task();
        Sample { footprint, resident, metal: metal(), accelerator: accelerator() }
    }
}
