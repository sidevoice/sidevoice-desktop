//! iOS: `UIDevice.current.name`. Since iOS 16 it is the generic model ("iPhone") unless the app holds the
//! user-assigned-device-name entitlement; that is the name the page proposes.
use super::Os;
use objc2::MainThreadMarker;
use objc2_ui_kit::UIDevice;

pub struct Native;

impl Os for Native {
    fn device_name() -> Option<String> {
        // `UIDevice` is main-thread only. The window is built in the app's setup, on the main thread.
        let main = MainThreadMarker::new()?;
        Some(UIDevice::currentDevice(main).name().to_string())
    }
}
