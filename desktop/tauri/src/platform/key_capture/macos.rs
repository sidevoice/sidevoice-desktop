//! macOS: a listen-only event tap on `flagsChanged` (Fn, the right-hand modifiers) and `keyDown` (a key pressed while one is
//! down), made only while a single key is set and Input Monitoring is granted; and a local monitor of the app's own events
//! for recording a shortcut, which needs no permission.

use super::{CaptureSink, KeyEvent, Os, Sink};
use block2::RcBlock;
use core_foundation::runloop::{kCFRunLoopCommonModes, kCFRunLoopDefaultMode, CFRunLoop, CFRunLoopRunResult};
use core_graphics::event::{
    CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType, CallbackResult, EventField,
};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType};
use sidevoice_desktop_core::shortcuts::{Heard, Mods, SingleKey};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;
use tauri::{AppHandle, Runtime};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
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
        let flag = stop.clone();
        std::thread::Builder::new().name("single-key-listener".into()).spawn(move || run(sink, flag, ready_tx)).ok()?;
        ready_rx.recv().ok()?.then(|| Box::new(Stop(stop)) as Box<dyn Send>)
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

/// Stops the listener's thread, which then lets go of the tap.
struct Stop(Arc<AtomicBool>);

impl Drop for Stop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// The listener's thread: the tap, on this thread's run loop, until told to stop. Whether it started goes to `ready`.
fn run(sink: Sink, stop: Arc<AtomicBool>, ready: mpsc::Sender<bool>) {
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
                        Some(key) => KeyEvent::Single { key, down: key.macos_down(event.get_flags().bits()) },
                        None => KeyEvent::Other,
                    });
                }
                CGEventType::KeyDown => sink(KeyEvent::Other),
                _ => {}
            }
            CallbackResult::Keep
        },
    );
    let Ok(tap) = tap else {
        let _ = ready.send(false);
        return;
    };
    let Ok(source) = tap.mach_port().create_runloop_source(0) else {
        let _ = ready.send(false);
        return;
    };
    // SAFETY: the run loop mode constants are the system's own, valid for the life of the process.
    let (common, default) = unsafe { (kCFRunLoopCommonModes, kCFRunLoopDefaultMode) };
    CFRunLoop::get_current().add_source(&source, common);
    tap.enable();
    let _ = ready.send(true);
    while !stop.load(Ordering::SeqCst) {
        if CFRunLoop::run_in_mode(default, Duration::from_secs(1), false) == CFRunLoopRunResult::Finished {
            std::thread::sleep(Duration::from_millis(100));
        }
        if disabled.swap(false, Ordering::SeqCst) {
            tap.enable();
        }
    }
    CFRunLoop::get_current().remove_source(&source, common);
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
                None => return event.as_ptr(),
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
