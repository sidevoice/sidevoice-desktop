//! macOS: the session dictionary the window server keeps says whether the screen is locked.

use super::Os;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::CFString;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
}

pub(super) struct Native;

impl Os for Native {
    fn screen_locked() -> bool {
        // SAFETY: the copy is owned (create rule) and released by the wrapper; a null is no session.
        let dictionary = unsafe {
            let copy = CGSessionCopyCurrentDictionary();
            if copy.is_null() {
                return false;
            }
            CFDictionary::<CFString, CFType>::wrap_under_create_rule(copy)
        };
        let key = CFString::from_static_string("CGSSessionScreenIsLocked");
        dictionary.find(&key).and_then(|value| value.downcast::<CFBoolean>()).is_some_and(bool::from)
    }
}
