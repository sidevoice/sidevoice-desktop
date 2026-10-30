use super::{Cleanup, Os, PointerEvent, Sink};
use crate::debug;
use sidevoice_desktop_core::call_controls as logic;
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};
use tauri::AppHandle;
use tauri_nspanel::objc2::{class, msg_send, rc::Retained, runtime::AnyObject};

// AppKit objects stay on their owning thread; only an opaque numeric cleanup key crosses threads.
thread_local! { static MONITORS: RefCell<HashMap<u64, Retained<AnyObject>>> = RefCell::new(HashMap::new()); }
static NEXT_MONITOR: AtomicU64 = AtomicU64::new(1);
pub struct Native;

/// macOS: outside clicks come from a global NSEvent monitor, and the app sets the cursor itself.
impl Os for Native {
    fn prepare() {
        set_cursor_in_background();
    }

    /// The hand over the buttons, the arrow once the pointer has left the card.
    fn cursor_upkeep(inside: Option<bool>, page_wants_hand: bool, was_inside: Option<bool>) {
        keep_hand(inside, page_wants_hand);
        if was_inside != inside && inside == Some(false) {
            arrow_after_leaving();
        }
    }

    fn click_monitor(app: &AppHandle, stopped: Arc<AtomicBool>, sink: Sink) -> Option<Cleanup> {
        let handler = block2::RcBlock::new(move |_event: std::ptr::NonNull<AnyObject>| {
            if !stopped.load(Ordering::Acquire) {
                sink(PointerEvent::OutsideClick);
            }
        });
        // NSEvent copies the block until removeMonitor; no leaked RcBlock is needed.
        const MOUSE_DOWN: u64 = (1 << 1) | (1 << 3) | (1 << 25);
        let monitor: Option<Retained<AnyObject>> = unsafe {
            msg_send![class!(NSEvent), addGlobalMonitorForEventsMatchingMask: MOUSE_DOWN, handler: &*handler]
        };
        debug(&format!("call-controls watching clicks elsewhere: {}", monitor.is_some()));
        let monitor = monitor?;
        let id = NEXT_MONITOR.fetch_add(1, Ordering::Relaxed);
        MONITORS.with(|monitors| monitors.borrow_mut().insert(id, monitor));
        let app = app.clone();
        Some(Box::new(move || {
            let _ = app.run_on_main_thread(move || {
                if let Some(monitor) = MONITORS.with(|monitors| monitors.borrow_mut().remove(&id)) {
                    // SAFETY: the retained NSEvent monitor, removed on the main thread before releasing it.
                    unsafe {
                        let _: () = msg_send![class!(NSEvent), removeMonitor: &*monitor];
                    }
                }
            });
        }))
    }
}

/// macOS: lets the app set the cursor while it is not the active app (#139). The window server ignores a cursor set
/// by an app in the background, so the hand over the card's buttons never showed; `SetsCursorInBackground` is the
/// connection property it honours instead. Private, looked up at run time: where it is gone the card only keeps the
/// arrow. WebKit sets a cursor only while the pointer is over its own window, and the app puts the arrow back when the
/// pointer leaves the card ([`arrow_after_leaving`]).
fn set_cursor_in_background() {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::string::CFString;
    type MainConnection = unsafe extern "C" fn() -> i32;
    type SetProperty = unsafe extern "C" fn(i32, i32, *const std::ffi::c_void, *const std::ffi::c_void) -> i32;
    let symbol = |name: &std::ffi::CStr| unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    let main = symbol(c"CGSMainConnectionID");
    let set = symbol(c"CGSSetConnectionProperty");
    let result = if main.is_null() || set.is_null() {
        "missing".to_string()
    } else {
        // SAFETY: CoreGraphics' own functions, with the signatures it has had since Mac OS X 10.4.
        unsafe {
            let main = std::mem::transmute::<*mut std::ffi::c_void, MainConnection>(main);
            let set = std::mem::transmute::<*mut std::ffi::c_void, SetProperty>(set);
            let connection = main();
            let key = CFString::from_static_string("SetsCursorInBackground");
            let error = set(
                connection,
                connection,
                key.as_concrete_TypeRef().cast(),
                CFBoolean::true_value().as_concrete_TypeRef().cast(),
            );
            if error == 0 {
                "true".to_string()
            } else {
                format!("error {error}")
            }
        }
    };
    debug(&format!("call-controls sets the cursor in background: {result}"));
}

/// macOS: the pointer left the card: the cursor it showed there (a hand over a button) goes back to the arrow, since the
/// app under the pointer may not set its own until the pointer enters one of its views again.
fn arrow_after_leaving() {
    {
        use tauri_nspanel::objc2::{class, msg_send, runtime::AnyObject};
        // SAFETY: NSCursor's class methods, on the main thread.
        unsafe {
            let current: *mut AnyObject = msg_send![class!(NSCursor), currentCursor];
            let arrow: *mut AnyObject = msg_send![class!(NSCursor), arrowCursor];
            if current != arrow {
                let _: () = msg_send![arrow, set];
            }
        }
    }
}

/// macOS: the hand over the card's buttons while another app is in front (#166). WebKit sets the card's cursor only when
/// the pointer moves over it, and an app in front that sets its own cursor on every mouse move (a text editor's I-beam)
/// takes it back at once, so the hand is set again at each look at the pointer ([`logic::must_set_hand`]).
fn keep_hand(inside: Option<bool>, page_wants_hand: bool) {
    if inside != Some(true) || !page_wants_hand {
        return;
    }
    {
        use tauri_nspanel::objc2::{class, msg_send, runtime::AnyObject};
        // SAFETY: NSCursor's class methods, on the main thread.
        unsafe {
            let current: *mut AnyObject = msg_send![class!(NSCursor), currentCursor];
            let hand: *mut AnyObject = msg_send![class!(NSCursor), pointingHandCursor];
            if logic::must_set_hand(inside, page_wants_hand, current == hand) {
                let _: () = msg_send![hand, set];
            }
        }
    }
}
