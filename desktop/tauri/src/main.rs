#![cfg(desktop)]
// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let first = std::env::args_os().nth(1);
    if first.as_deref() == Some(std::ffi::OsStr::new("--sidevoice-reset-helper")) {
        sidevoice_desktop_lib::run_reset_helper();
        return;
    }
    // The app's own executable, started by the app to ask the system afresh about System Audio Recording.
    if first.as_deref() == Some(std::ffi::OsStr::new(sidevoice_desktop_core::media_duck::CAPTURE_CHECK_FLAG)) {
        sidevoice_desktop_lib::run_capture_check();
        return;
    }
    sidevoice_desktop_lib::run()
}
