//! What the app says about itself when asked to: the one switch (`SIDEVOICE_DEBUG=1`) and the two ways to say it.

/// Whether `SIDEVOICE_DEBUG=1`: diagnostics on stderr, and what CI's smoke tests read.
pub fn enabled() -> bool {
    std::env::var_os("SIDEVOICE_DEBUG").is_some_and(|v| v == "1")
}

/// Diagnostics on stderr, only with `SIDEVOICE_DEBUG=1` (CI's smoke tests read them).
#[allow(clippy::disallowed_macros, reason = "the SIDEVOICE_DEBUG=1 lines CI's smoke tests read, byte for byte")]
pub fn debug(message: &str) {
    tracing::info!("{message}");
    if enabled() {
        eprintln!("sidevoice: {message}");
    }
}

/// A line too frequent or too large for the log by default: in it at level `debug` (`SIDEVOICE_LOG`), and on stderr
/// with `SIDEVOICE_DEBUG=1`.
#[allow(clippy::disallowed_macros, reason = "the SIDEVOICE_DEBUG=1 lines CI's smoke tests read, byte for byte")]
pub fn debug_detail(message: impl FnOnce() -> String) {
    if !enabled() && !tracing::enabled!(tracing::Level::DEBUG) {
        return;
    }
    let message = message();
    tracing::debug!("{message}");
    if enabled() {
        eprintln!("sidevoice: {message}");
    }
}
