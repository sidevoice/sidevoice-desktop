//! The iOS body: the same MediaPlayer code as macOS (`apple.rs`): the call on the lock screen and in Control Center, its
//! play/pause from there and from a headset. iOS takes the playback state from the app's audio session, so the app sets
//! none. The AirPods mute gesture (iOS 17 and later): iOS mutes the app's input itself and posts
//! `AVAudioApplicationInputMuteStateChangeNotification`, which the app observes to bring the call in step.
//!
//! The app runs on iOS 15 and 16 too, where `AVAudioApplication` and its notification do not exist: the class is looked
//! up at run time (`apple.rs` asks before anything here runs), and the notification's name and its key are read with
//! `dlsym`, so that the app does not link symbols an older iOS lacks (a missing one would stop it from launching).
use super::{CallState, CommandSink, Os};
use block2::RcBlock;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObjectProtocol, ProtocolObject};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSNumber, NSOperationQueue, NSString};
use std::ffi::{c_void, CStr};
use std::ptr::NonNull;
use std::sync::Mutex;
use tauri::AppHandle;

#[path = "apple.rs"]
mod apple;

pub(super) struct Native;

impl Os for Native {
    const NATIVE: bool = true;

    fn attach(app: &AppHandle, sink: CommandSink) {
        apple::attach::<Native>(app, sink)
    }

    fn update(app: &AppHandle, call: &CallState) {
        apple::update::<Native>(app, call)
    }
}

/// Darwin's `RTLD_DEFAULT`: every image loaded in the process.
const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

/// The `NSString *` constant AVFAudio exports as `symbol`, when this iOS has it.
fn exported_string(symbol: &CStr) -> Option<&'static NSString> {
    // SAFETY: `dlsym` only looks the name up; a constant AVFAudio exports is an `NSString *const`, valid for the process's
    // life.
    unsafe {
        let address = dlsym(RTLD_DEFAULT, symbol.as_ptr()) as *const *const NSString;
        address.as_ref().and_then(|string| string.as_ref())
    }
}

extern "C" {
    /// libSystem's, which every iOS app links.
    fn dlsym(handle: *mut c_void, symbol: *const std::ffi::c_char) -> *mut c_void;
}

/// The observer while it is installed (only during a call). Only touched on the main thread.
struct Observer(Retained<ProtocolObject<dyn NSObjectProtocol>>);
// SAFETY: installed and removed on the main thread only (`apple::update` dispatches there); the mutex only holds it.
unsafe impl Send for Observer {}
static OBSERVER: Mutex<Option<Observer>> = Mutex::new(None);

impl apple::Os for Native {
    fn listen_for_mute_gesture(app: &AppHandle) -> bool {
        let (Some(name), Some(key)) = (
            exported_string(c"AVAudioApplicationInputMuteStateChangeNotification"),
            exported_string(c"AVAudioApplicationMuteStateKey"),
        ) else {
            return false;
        };
        let app = app.clone();
        let block = RcBlock::new(move |notification: NonNull<NSNotification>| {
            // SAFETY: the notification is valid for the block's call; its user info holds the state as an NSNumber.
            let muted = unsafe {
                notification
                    .as_ref()
                    .userInfo()
                    .and_then(|info| info.objectForKey(key))
                    .and_then(|value| value.downcast::<NSNumber>().ok())
                    .map(|number| number.boolValue())
            };
            if let Some(muted) = muted {
                apple::heard(&app, muted);
            }
        });
        // SAFETY: main thread; the center retains the block and returns the token that removes it.
        let token = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(name),
                None::<&AnyObject>,
                Some(&NSOperationQueue::mainQueue()),
                &block,
            )
        };
        *OBSERVER.lock().unwrap() = Some(Observer(token));
        true
    }

    /// `-[UIApplication endReceivingRemoteControlEvents]`: without it the app can stay the Now Playing app (an empty card
    /// on the Lock Screen) after its handlers are gone.
    fn end_remote_control_events() {
        let Some(class) = AnyClass::get(c"UIApplication") else { return };
        // SAFETY: main thread (`apple` calls it there); `+sharedApplication` returns the app (nil before it exists), and
        // `-endReceivingRemoteControlEvents` takes and returns nothing.
        unsafe {
            let application: *mut AnyObject = msg_send![class, sharedApplication];
            if let Some(application) = application.as_ref() {
                let _: () = msg_send![application, endReceivingRemoteControlEvents];
            }
        }
    }

    fn stop_listening_for_mute_gesture() {
        if let Some(Observer(token)) = OBSERVER.lock().unwrap().take() {
            // SAFETY: main thread; the token is the one the center returned.
            let observer: &AnyObject = AsRef::<AnyObject>::as_ref(&*token);
            unsafe { NSNotificationCenter::defaultCenter().removeObserver(observer) };
        }
    }
}
