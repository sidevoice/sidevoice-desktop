use std::cell::RefCell;
use std::ptr::NonNull;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2::MainThreadMarker;
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNotificationName, NSOperationQueue,
};
use objc2_ui_kit::{
    UIApplication, UIApplicationDidBecomeActiveNotification,
    UIApplicationProtectedDataDidBecomeAvailable, UIApplicationProtectedDataWillBecomeUnavailable,
};
use tauri::{AppHandle, Runtime};

use super::Os;
use crate::ScreenError;

/// `UIApplication.isIdleTimerDisabled` to keep the screen on, and the protected data for the lock: UIKit's, from the main
/// thread. iOS has no public answer to "is the screen locked" or "is it off"; what it says is whether the files it protects
/// can be read (`isProtectedDataAvailable`), which they cannot once the phone has locked, with a passcode, and only some
/// seconds after the lock (about 10, when the class keys are discarded; some more after a recent unlock). Without a passcode
/// the phone never reads as locked. No private notification of the system's (SpringBoard's lock state) is read.
pub struct Native;

thread_local! {
    /// The notification center's tokens of the watch's observers, kept for the app's life, on the main thread.
    static OBSERVERS: RefCell<Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>> = const { RefCell::new(Vec::new()) };
}

/// Whether the protected data is gone: the phone has locked.
fn gone(main: MainThreadMarker) -> bool {
    !UIApplication::sharedApplication(main).isProtectedDataAvailable()
}

/// `changed` hears `locked` (or, for `None`, the protected data's state as UIKit says it then) on each `name`, on the main
/// queue.
fn observe(name: &NSNotificationName, locked: Option<bool>, changed: fn(bool)) {
    let block = RcBlock::new(move |_: NonNull<NSNotification>| {
        changed(locked.unwrap_or_else(|| MainThreadMarker::new().is_some_and(gone)));
    });
    // SAFETY: the main queue runs the block on the main thread; it holds a flag and a plain function, and asks UIKit only there.
    let token = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(name),
            None,
            Some(&NSOperationQueue::mainQueue()),
            &block,
        )
    };
    OBSERVERS.with(|observers| observers.borrow_mut().push(token));
}

/// The watch's observers: the protected data going (the phone locked) and coming back (unlocked), and the app becoming
/// active, when it is read (the first time as the app launches; again after a change the app missed while suspended, outside
/// a call). Only Foundation's notification center here, and no reading: the app's plugins are set up before UIKit's
/// application exists (Tauri's `build`, before `run` enters `UIApplicationMain`), and asking it then ends the app.
fn watch_here(changed: fn(bool)) {
    // SAFETY: UIKit's notification names, constant strings for the process's life.
    let (will_lock, unlocked, active) = unsafe {
        (
            UIApplicationProtectedDataWillBecomeUnavailable,
            UIApplicationProtectedDataDidBecomeAvailable,
            UIApplicationDidBecomeActiveNotification,
        )
    };
    observe(will_lock, Some(true), changed);
    observe(unlocked, Some(false), changed);
    observe(active, None, changed);
}

impl Os for Native {
    fn keep_on<R: Runtime>(app: &AppHandle<R>, on: bool) -> Result<(), ScreenError> {
        app.run_on_main_thread(move || {
            if let Some(main) = MainThreadMarker::new() {
                UIApplication::sharedApplication(main).setIdleTimerDisabled(on);
            }
        })
        .map_err(|error| ScreenError(error.to_string()))
    }

    /// Read at once on the main thread; from another, asked there and waited for (a second at most, else not locked).
    fn locked<R: Runtime>(app: &AppHandle<R>) -> bool {
        if let Some(main) = MainThreadMarker::new() {
            return gone(main);
        }
        let (answer, answered) = std::sync::mpsc::sync_channel(1);
        let asked = app.run_on_main_thread(move || {
            if let Some(main) = MainThreadMarker::new() {
                let _ = answer.send(gone(main));
            }
        });
        asked.is_ok()
            && answered
                .recv_timeout(Duration::from_secs(1))
                .unwrap_or(false)
    }

    fn watch<R: Runtime>(app: &AppHandle<R>, changed: fn(bool)) {
        if MainThreadMarker::new().is_some() {
            watch_here(changed);
        } else if let Err(error) = app.run_on_main_thread(move || watch_here(changed)) {
            log::warn!(target: "cue", "screen: the lock is not watched: {error}");
        }
    }
}
