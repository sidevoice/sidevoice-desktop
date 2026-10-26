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

/// The footprint and the resident set, in bytes.
fn task() -> (Option<u64>, Option<u64>) {
    use mach2::kern_return::KERN_SUCCESS;
    use mach2::message::mach_msg_type_number_t;
    use mach2::task::task_info;
    use mach2::task_info::{task_info_t, task_vm_info, TASK_VM_INFO};
    use mach2::traps::mach_task_self;
    use std::mem::{offset_of, size_of};

    // The kernel fills as many 32-bit words as it has (the struct grows between releases); the rest stays zero.
    let mut info = task_vm_info::default();
    let mut count = (size_of::<task_vm_info>() / size_of::<u32>()) as mach_msg_type_number_t;
    // SAFETY: `info` has room for `count` 32-bit words; `mach_task_self` is this process's task port.
    let status =
        unsafe { task_info(mach_task_self(), TASK_VM_INFO, (&raw mut info).cast::<i32>() as task_info_t, &mut count) };
    // An older kernel stops before `phys_footprint`: nothing to report then.
    if status != KERN_SUCCESS || (count as usize) * size_of::<u32>() < offset_of!(task_vm_info, phys_footprint) + 8 {
        return (None, None);
    }
    (Some(info.phys_footprint), Some(info.resident_size))
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
