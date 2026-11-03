//! Where the call's surfaces read the creatures' stills from (`crate::call_surface`): iOS shares a container with the
//! widget extension that draws the Live Activity (the App Group's); Android's notification is drawn by the app itself, and
//! has none. Each OS file implements `Os`; the common code is `crate::call_surface`.
use std::path::PathBuf;

#[cfg_attr(target_os = "android", path = "call_surface/android.rs")]
#[cfg_attr(target_os = "ios", path = "call_surface/ios.rs")]
mod imp;
use imp::Native;

trait Os {
    /// The folder the app shares with its extensions, where the system gives it one.
    fn shared_container() -> Option<PathBuf> {
        None
    }
}

pub fn shared_container() -> Option<PathBuf> {
    Native::shared_container()
}
