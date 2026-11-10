//! The iOS body over the macOS code (`apple.rs`), with no Now Playing: during a call a Bluetooth headset is the call's
//! hands-free (HFP), and its presses are call control, which iOS gives to CallKit and never to the remote-command center
//! (desktop#464); Now Playing would only make the app look like a music source to a car (desktop#465). So what reaches the app
//! is the AirPods / Beats mute gesture, "Press to Mute and Unmute" (iOS 17 and later): an app without CallKit opts in by
//! observing `AVAudioApplicationInputMuteStateChangeNotification` (Apple, WWDC23 "Enhance your app's audio experience with
//! AirPods"), which this app does once, at launch, for the process's life (`apple::observe_input_mute`, as macOS does). iOS then
//! mutes the app's input itself, plays its tone and posts the notification, which brings the call in step; the app's own mute
//! goes the other way through `setInputMuted`. It is posted only while a record session is active (the call's
//! play-and-record, voice chat: `mobile/tauri` `call.rs`).
//!
//! The app runs on iOS 15 and 16 too, where `AVAudioApplication` and its notification do not exist: the class is looked
//! up at run time (`apple.rs` asks before anything here runs), and so are the notification's name and its key (`apple.rs`).
use super::{CallState, CommandSink, Os};
use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject};
use sidevoice_shared_headset::HeadsetCapabilities;
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

impl apple::Os for Native {
    /// The notification is both the opt-in and how the gesture arrives: iOS has muted the input already, and its state is
    /// the call's single press.
    fn listen_for_mute_gesture(app: &AppHandle) -> bool {
        let app = app.clone();
        apple::observe_input_mute(move |muted| {
            apple::heard(&app, muted);
        })
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
}
