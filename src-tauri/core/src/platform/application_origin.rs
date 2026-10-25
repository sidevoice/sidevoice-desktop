/// The origin Tauri serves the app's own pages from: what a node sees as the interface's `Origin`.
#[cfg(windows)]
pub const APP_ORIGIN: &str = "http://tauri.localhost";
#[cfg(not(windows))]
pub const APP_ORIGIN: &str = "tauri://localhost";
