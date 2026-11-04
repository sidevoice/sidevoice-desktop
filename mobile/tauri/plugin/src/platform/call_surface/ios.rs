//! iOS: the App Group's container, and the call's Live Activity, which the app's Swift starts, updates and ends
//! (`SidevoiceCallActivity`, mobile/tauri/apple/CallActivity/Sources/CallActivityApp). Its buttons are App Intents that
//! run in the app and ask for the page's actions through `SidevoiceCallSurfaceActions`, defined here.
use std::path::PathBuf;
use std::sync::Mutex;

use objc2::runtime::AnyClass;
use objc2::{define_class, msg_send, ClassType};
use objc2_foundation::{NSFileManager, NSObject, NSObjectProtocol, NSString};

use super::{OnAction, Os};

/// The App Group the app shares with its Live Activity's widget extension (both targets carry it in their entitlements).
const APP_GROUP: &str = "group.ai.sidevoice.mobile";

/// Where the actions asked for on the running activity go: none while no activity runs.
static ON_ACTION: Mutex<Option<OnAction>> = Mutex::new(None);

pub struct Native;

impl Os for Native {
    /// The App Group's container (`containerURLForSecurityApplicationGroupIdentifier:`): none while the app is not
    /// entitled to the group.
    fn shared_container() -> Option<PathBuf> {
        let manager = NSFileManager::defaultManager();
        let url = manager
            .containerURLForSecurityApplicationGroupIdentifier(&NSString::from_str(APP_GROUP))?;
        url.to_file_path()
    }

    fn start(details: &str, on_action: OnAction) {
        // Registered before the activity's first button can ask for it.
        let _ = <SurfaceActions as ClassType>::class();
        if let Ok(mut current) = ON_ACTION.lock() {
            *current = Some(on_action);
        }
        let Some(bridge) = bridge() else { return };
        let details = NSString::from_str(details);
        // SAFETY: `+[SidevoiceCallActivity start:]` takes a string and returns nothing (CallActivityBridge.swift).
        unsafe {
            let _: () = msg_send![bridge, start: &*details];
        }
    }

    fn show(details: &str) {
        let Some(bridge) = bridge() else { return };
        let details = NSString::from_str(details);
        // SAFETY: `+[SidevoiceCallActivity show:]`, as above.
        unsafe {
            let _: () = msg_send![bridge, show: &*details];
        }
    }

    fn end() {
        if let Ok(mut current) = ON_ACTION.lock() {
            *current = None;
        }
        let Some(bridge) = bridge() else { return };
        // SAFETY: `+[SidevoiceCallActivity end]` takes nothing and returns nothing.
        unsafe {
            let _: () = msg_send![bridge, end];
        }
    }

    fn running() -> bool {
        // Not `bridge()`: this is asked on every report, and a build without the bridge has said so once.
        let Some(bridge) = AnyClass::get(c"SidevoiceCallActivity") else {
            return false;
        };
        // SAFETY: `+[SidevoiceCallActivity running]` takes nothing and returns a BOOL, from any thread.
        unsafe { msg_send![bridge, running] }
    }
}

/// The app's Swift side of the activity; none in a build without it (logged).
fn bridge() -> Option<&'static AnyClass> {
    let bridge = AnyClass::get(c"SidevoiceCallActivity");
    if bridge.is_none() {
        log::warn!("call surface: the app has no SidevoiceCallActivity: no Live Activity");
    }
    bridge
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class has no ivars and does not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "SidevoiceCallSurfaceActions"]
    struct SurfaceActions;

    unsafe impl NSObjectProtocol for SurfaceActions {}

    impl SurfaceActions {
        /// A button of the Live Activity asked for `action` (`CallActions.send`, from the intent's `perform`, on any
        /// thread).
        #[unsafe(method(act:))]
        fn act(action: &NSString) {
            let on_action = ON_ACTION.lock().ok().and_then(|current| current.clone());
            match on_action {
                Some(on_action) => on_action(&action.to_string()),
                None => log::warn!("call surface: {action} asked for with no call running"),
            }
        }
    }
);
