// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--sidevoice-reset-helper")) {
        let parent = std::env::args().nth(2).and_then(|arg| arg.parse().ok());
        sidevoice_desktop_lib::run_reset_helper(parent);
        return;
    }
    sidevoice_desktop_lib::run()
}
