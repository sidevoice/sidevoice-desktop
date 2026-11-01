/// The app's entry point for the native side: Android's activity loads the library and calls it, iOS's `main` calls
/// `start_app`. Tauri's macro writes that glue, and exists only for the OSes it serves, so it is named by them.
#[cfg_attr(
    any(target_os = "android", target_os = "ios"),
    tauri::mobile_entry_point
)]
pub fn run() {
    crate::run_app();
}
