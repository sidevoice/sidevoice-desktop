//! Android: logcat, tag `sidevoice`, the priority of the level. A line longer than logcat's entry is written in pieces.
use std::ffi::{c_int, CString};

use android_log_sys::{__android_log_write, LogPriority};
use tracing::Level;

use super::Os;

const TAG: &std::ffi::CStr = c"sidevoice";

/// What one logcat entry holds, less a margin (the limit is 4068 bytes, the tag and the priority included).
const ENTRY: usize = 3500;

pub struct Native;

impl Os for Native {
    fn write(_category: &str, level: Level, line: &str) {
        let priority = match level {
            Level::ERROR => LogPriority::ERROR,
            Level::WARN => LogPriority::WARN,
            Level::INFO => LogPriority::INFO,
            Level::DEBUG => LogPriority::DEBUG,
            Level::TRACE => LogPriority::VERBOSE,
        };
        let mut rest = line;
        while !rest.is_empty() {
            let mut end = rest.len().min(ENTRY);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            let (piece, tail) = rest.split_at(end);
            // A line has no NUL: tracing's text is Rust's, and a field holding one is cut at it.
            if let Ok(text) = CString::new(piece.replace('\0', "")) {
                // SAFETY: the tag and the text are NUL-terminated and live through the call.
                unsafe {
                    __android_log_write(priority as c_int, TAG.as_ptr(), text.as_ptr());
                }
            }
            rest = tail;
        }
    }
}
