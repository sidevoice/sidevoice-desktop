//! `exit()` runs the handlers registered with `atexit` and the static destructors (`__cxa_atexit`, when each static is
//! built) from one list, the last registered first. Registered before whisper.cpp's Metal device is built, this runs
//! after its destructor, which aborts when a model is still loaded (#121).

use super::Native;

pub(super) struct Os;

impl Native for Os {
    fn report_handlers_ran() {
        extern "C" fn ran() {
            eprintln!("sidevoice: probe: exit handlers ran");
        }
        // SAFETY: registers a plain function, which only writes to stderr.
        unsafe { libc::atexit(ran) };
    }
}
