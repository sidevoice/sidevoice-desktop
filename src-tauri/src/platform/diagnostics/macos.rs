//! `exit()` runs the handlers registered with `atexit` and the static destructors (`__cxa_atexit`, when each static is
//! built) from one list, the last registered first. Registered before whisper.cpp's Metal device is built, the exit
//! handler runs after its destructor, which aborts when a model is still loaded (#121).

use super::Os;
#[cfg(sidevoice_debug_options)]
use sidevoice_desktop_core::debug_options::{DebugOptions, Switch};

pub(super) struct Native;

impl Os for Native {
    #[cfg(feature = "probe")]
    fn report_handlers_ran() {
        extern "C" fn ran() {
            eprintln!("sidevoice: probe: exit handlers ran");
        }
        // SAFETY: registers a plain function, which only writes to stderr.
        unsafe { libc::atexit(ran) };
    }

    /// The physical footprint, from the task's VM info.
    #[cfg(any(test, feature = "probe"))]
    fn footprint() -> Option<u64> {
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
        let status = unsafe {
            task_info(mach_task_self(), TASK_VM_INFO, (&raw mut info).cast::<i32>() as task_info_t, &mut count)
        };
        // An older kernel stops before `phys_footprint`: nothing to report then.
        if status != KERN_SUCCESS || (count as usize) * size_of::<u32>() < offset_of!(task_vm_info, phys_footprint) + 8
        {
            return None;
        }
        Some(info.phys_footprint)
    }

    /// What the process holds on the GPU through Metal: `currentAllocatedSize` of the default device. `None` with no
    /// GPU.
    #[cfg(any(test, feature = "probe"))]
    fn metal() -> Option<u64> {
        use objc2::msg_send;
        use objc2::rc::Retained;
        use objc2::runtime::AnyObject;

        #[link(name = "Metal", kind = "framework")]
        extern "C" {
            /// The system's default GPU; null where there is none. Owned by the caller (a "create" function).
            fn MTLCreateSystemDefaultDevice() -> *mut AnyObject;
        }

        // SAFETY: a plain C function; the device it returns is released below, once.
        let device = unsafe { Retained::from_raw(MTLCreateSystemDefaultDevice()) }?;
        // SAFETY: an MTLDevice answers `currentAllocatedSize` with an NSUInteger.
        let size: usize = unsafe { msg_send![&*device, currentAllocatedSize] };
        Some(size as u64)
    }

    /// The GPU's mapped regions, read by the system's own `vmmap` on this process (it can take a second or two).
    #[cfg(any(test, feature = "probe"))]
    fn accelerator() -> Option<u64> {
        let out = std::process::Command::new("/usr/bin/vmmap")
            .args(["-summary", &std::process::id().to_string()])
            .output()
            .ok()?;
        sidevoice_desktop_core::memory::accelerator_bytes(&String::from_utf8_lossy(&out.stdout))
    }

    #[cfg(sidevoice_debug_options)]
    fn webview_switches() -> Vec<Switch> {
        use objc2::runtime::AnyClass;
        use objc2::sel;

        let has = |name, selector| AnyClass::get(name).is_some_and(|class| class.instance_method(selector).is_some());
        [
            (Switch::Inspectable, has(c"WKWebView", sel!(setInspectable:))),
            (Switch::CompositingBorders, has(c"WKPreferences", sel!(_setCompositingBordersVisible:))),
            (Switch::RepaintCounters, has(c"WKPreferences", sel!(_setCompositingRepaintCountersVisible:))),
        ]
        .into_iter()
        .filter_map(|(switch, present)| present.then_some(switch))
        .collect()
    }

    #[cfg(sidevoice_debug_options)]
    fn apply_webview_switches(window: &tauri::WebviewWindow, options: DebugOptions) -> Result<(), String> {
        use objc2::runtime::{AnyObject, Bool};
        use objc2::{msg_send, sel};

        window
            .with_webview(move |webview| {
                let view = webview.inner() as *mut AnyObject;
                if view.is_null() {
                    return;
                }
                // SAFETY: the window's own WKWebView and its live preferences, on the main thread (`with_webview`); each setter
                // only where the system answers to it.
                unsafe {
                    let responds: bool = msg_send![view, respondsToSelector: sel!(setInspectable:)];
                    if responds {
                        let _: () = msg_send![view, setInspectable: Bool::new(options.inspectable)];
                    }
                    let configuration: *mut AnyObject = msg_send![view, configuration];
                    if configuration.is_null() {
                        return;
                    }
                    let preferences: *mut AnyObject = msg_send![configuration, preferences];
                    if preferences.is_null() {
                        return;
                    }
                    let responds: bool = msg_send![preferences, respondsToSelector: sel!(_setCompositingBordersVisible:)];
                    if responds {
                        let _: () =
                            msg_send![preferences, _setCompositingBordersVisible: Bool::new(options.compositing_borders)];
                    }
                    let responds: bool =
                        msg_send![preferences, respondsToSelector: sel!(_setCompositingRepaintCountersVisible:)];
                    if responds {
                        let _: () =
                            msg_send![preferences, _setCompositingRepaintCountersVisible: Bool::new(options.repaint_counters)];
                    }
                }
            })
            .map_err(|error| error.to_string())
    }
}
