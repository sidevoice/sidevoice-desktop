//! macOS: a listen-only event tap on `flagsChanged` (Fn, the right-hand modifiers) and `keyDown` (a key pressed while one is
//! down), made only while a single key is set and Input Monitoring is granted; and a local monitor of the app's own events
//! for recording a shortcut, which needs no permission.

use super::{CaptureSink, Os, Sink};
use block2::RcBlock;
use core_foundation::base::TCFType;
use core_foundation::mach_port::CFMachPortRef;
use core_foundation::runloop::{kCFRunLoopCommonModes, kCFRunLoopDefaultMode, CFRunLoop, CFRunLoopRunResult};
use core_graphics::event::{
    CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType, CallbackResult, EventField,
};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType};
use sidevoice_desktop_core::shortcuts::{Heard, Listened, Mods, SingleKey};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;
use tauri::{AppHandle, Runtime};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFMachPortInvalidate(port: CFMachPortRef);
}

pub(super) struct Native;

impl Os for Native {
    const LISTENS: bool = true;
    const CAPTURES: bool = true;
    const SETTINGS_URL: Option<&'static str> =
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent");

    fn access() -> Option<bool> {
        // SAFETY: a plain query of the system's Input Monitoring state; it asks nothing of the person.
        Some(unsafe { CGPreflightListenEventAccess() })
    }

    fn request_access() -> bool {
        // SAFETY: shows the system's own prompt the first time; returns at once.
        unsafe { CGRequestListenEventAccess() }
    }

    fn listen(sink: Sink) -> Option<Box<dyn Send>> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let flag = stop.clone();
        std::thread::Builder::new()
            .name("single-key-listener".into())
            .spawn(move || run(sink, flag, ready_tx, done_tx))
            .ok()?;
        let run_loop = ready_rx.recv().ok()??;
        Some(Box::new(Stop { stop, run_loop, done }))
    }

    fn capture<R: Runtime>(app: &AppHandle<R>, sink: CaptureSink) -> Option<Box<dyn Send>> {
        let (tx, rx) = mpsc::channel();
        app.run_on_main_thread(move || {
            let _ = tx.send(monitor(sink).map(Token));
        })
        .ok()?;
        let token = rx.recv().ok()??;
        Some(Box::new(Capturing { app: app.clone(), token: Some(token) }))
    }
}

/// Stops the listener: the tap is off, invalidated and its thread gone when this is dropped (clearing the last single key
/// must not leave the keyboard listened to for the rest of an iteration of the run loop).
struct Stop {
    stop: Arc<AtomicBool>,
    /// The listener thread's run loop, to wake it at once.
    run_loop: CFRunLoop,
    /// Tells that the thread has torn the tap down.
    done: mpsc::Receiver<()>,
}

impl Drop for Stop {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // `CFRunLoopStop` may be called from any thread.
        self.run_loop.stop();
        // Bounded: a sink stuck in a callback must not hold the caller for good.
        let _ = self.done.recv_timeout(Duration::from_secs(2));
    }
}

/// The listener's thread: the tap, on this thread's run loop, until told to stop, then torn down here (disabled,
/// invalidated, its source removed) before `done` is told. Its run loop, once started, goes to `ready` (`None`: it could not).
fn run(sink: Sink, stop: Arc<AtomicBool>, ready: mpsc::Sender<Option<CFRunLoop>>, done: mpsc::Sender<()>) {
    let disabled = Arc::new(AtomicBool::new(false));
    let (was_disabled, stopped) = (disabled.clone(), stop.clone());
    let tap = CGEventTap::new(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![CGEventType::FlagsChanged, CGEventType::KeyDown],
        move |_proxy, kind, event| {
            if stopped.load(Ordering::SeqCst) {
                return CallbackResult::Keep;
            }
            match kind {
                // The system turns a listen-only tap off when the app is too slow, or on its own: it is turned back on.
                CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                    was_disabled.store(true, Ordering::SeqCst)
                }
                CGEventType::FlagsChanged => {
                    let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
                    sink(match SingleKey::from_macos_modifier(code) {
                        Some(key) => Listened::Single { key, down: key.macos_down(event.get_flags().bits()) },
                        None => Listened::Other,
                    });
                }
                CGEventType::KeyDown => sink(Listened::Other),
                _ => {}
            }
            CallbackResult::Keep
        },
    );
    let Ok(tap) = tap else {
        let _ = ready.send(None);
        return;
    };
    let Ok(source) = tap.mach_port().create_runloop_source(0) else {
        let _ = ready.send(None);
        return;
    };
    // SAFETY: the run loop mode constants are the system's own, valid for the life of the process.
    let (common, default) = unsafe { (kCFRunLoopCommonModes, kCFRunLoopDefaultMode) };
    CFRunLoop::get_current().add_source(&source, common);
    tap.enable();
    let _ = ready.send(Some(CFRunLoop::get_current()));
    while !stop.load(Ordering::SeqCst) {
        if CFRunLoop::run_in_mode(default, Duration::from_secs(1), false) == CFRunLoopRunResult::Finished {
            std::thread::sleep(Duration::from_millis(100));
        }
        if disabled.swap(false, Ordering::SeqCst) {
            tap.enable();
        }
    }
    // SAFETY: the port is the tap's own, alive until `tap` drops below; both calls are safe on any thread.
    unsafe {
        CGEventTapEnable(tap.mach_port().as_concrete_TypeRef(), false);
        CFMachPortInvalidate(tap.mach_port().as_concrete_TypeRef());
    }
    CFRunLoop::get_current().remove_source(&source, common);
    drop(tap);
    let _ = done.send(());
}

/// A local monitor of the app's own keyboard events: the token to take it off again.
struct Token(Retained<AnyObject>);

// SAFETY: only ever handed back to AppKit on the main thread (`Capturing::drop`).
unsafe impl Send for Token {}

struct Capturing<R: Runtime> {
    app: AppHandle<R>,
    token: Option<Token>,
}

impl<R: Runtime> Drop for Capturing<R> {
    fn drop(&mut self) {
        if let Some(token) = self.token.take() {
            let _ = self.app.run_on_main_thread(move || {
                // The whole token moves in (not just its field), which is what makes it `Send`.
                let token = token;
                // SAFETY: the token is the monitor `monitor` added, removed once, on the main thread.
                unsafe { NSEvent::removeMonitor(&token.0) };
            });
        }
    }
}

/// Hears the keys of the app's own windows, Fn included, and swallows them, so the page under the recorder gets none
/// (⌘M would minimise the window). Main thread.
fn monitor(sink: CaptureSink) -> Option<Retained<AnyObject>> {
    let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit passes a live event for the duration of the handler.
        let event_ref = unsafe { event.as_ref() };
        let flags = event_ref.modifierFlags().bits() as u64;
        let heard = match event_ref.r#type() {
            NSEventType::FlagsChanged => match SingleKey::from_macos_modifier(event_ref.keyCode()) {
                Some(key) => Heard::Modifier { key, down: key.macos_down(flags) },
                // A left-hand modifier cannot be a key on its own: the recorder says so, and goes on.
                None => match SingleKey::left_macos_down(event_ref.keyCode(), flags) {
                    Some(down) => Heard::LeftModifier { down },
                    None => return event.as_ptr(),
                },
            },
            NSEventType::KeyDown => {
                if event_ref.isARepeat() {
                    return std::ptr::null_mut();
                }
                Heard::Key {
                    code: event_ref.keyCode(),
                    character: event_ref.charactersIgnoringModifiers().map(|text| text.to_string()),
                    mods: Mods::from_macos_flags(flags),
                }
            }
            _ => return event.as_ptr(),
        };
        sink(heard);
        std::ptr::null_mut()
    });
    // SAFETY: the block returns the event it was given or null.
    unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown | NSEventMask::FlagsChanged, &block)
    }
}
