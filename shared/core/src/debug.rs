//! What the app says about itself when asked to: the one switch (`SIDEVOICE_DEBUG=1`) and the two ways to say it.
//!
//! These lines are stderr only, never the log: their text is built by the call site and may carry a page's url, a label or an
//! error's words, and the log is written as if it will be published (the Sidevoice logging guideline). What belongs in the
//! log is a `tracing` event with fields, next to the call.

/// Whether `SIDEVOICE_DEBUG=1`: diagnostics on stderr, and what CI's smoke tests read.
pub fn enabled() -> bool {
    std::env::var_os("SIDEVOICE_DEBUG").is_some_and(|v| v == "1")
}

/// Diagnostics on stderr, only with `SIDEVOICE_DEBUG=1` (CI's smoke tests read them).
#[allow(clippy::disallowed_macros, reason = "the SIDEVOICE_DEBUG=1 lines CI's smoke tests read, byte for byte")]
pub fn debug(message: &str) {
    if enabled() {
        eprintln!("sidevoice: {message}");
    }
}

/// A line too frequent or too large to build unless asked: on stderr with `SIDEVOICE_DEBUG=1`.
#[allow(clippy::disallowed_macros, reason = "the SIDEVOICE_DEBUG=1 lines CI's smoke tests read, byte for byte")]
pub fn debug_detail(message: impl FnOnce() -> String) {
    if enabled() {
        eprintln!("sidevoice: {}", message());
    }
}
