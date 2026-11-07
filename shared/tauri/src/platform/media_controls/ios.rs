//! The iOS body over the macOS code (`apple.rs`), with no Now Playing: during a call a Bluetooth headset is the call's
//! hands-free (HFP), and its presses are call control, which iOS gives to CallKit and never to the remote-command center
//! (desktop#464); Now Playing would only make the app look like a music source to a car (desktop#465). So what reaches the app
//! is the AirPods / Beats mute gesture, "Press to Mute and Unmute" (iOS 17 and later): an app without CallKit opts in by
//! observing `AVAudioApplicationInputMuteStateChangeNotification` (Apple, WWDC23 "Enhance your app's audio experience with
//! AirPods"), which this app does once, at launch, for the process's life. iOS then mutes the app's input itself, plays its tone
//! and posts the notification, which brings the call in step; the app's own mute goes the other way through `setInputMuted`.
//! It is posted only while a record session is active (the call's play-and-record, voice chat: `mobile/tauri` `call.rs`).
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
use sidevoice_shared_headset::HeadsetCapabilities;
use std::ffi::{c_void, CStr};
use std::ptr::NonNull;
use std::sync::Mutex;
use tauri::AppHandle;

#[path = "apple.rs"]
mod apple;

pub(super) struct Native;

impl Os for Native {
    const NATIVE: bool = true;

    /// Only the system's mute: the single press is Press to Mute (AirPods, Beats), and no double press reaches the app.
    const PRESSES: HeadsetCapabilities = HeadsetCapabilities::SYSTEM_MUTE;

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
    /// Opted in at launch: the notification observed before any call's audio session is active.
    const MUTE_GESTURE_FOR_LIFE: bool = true;

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
