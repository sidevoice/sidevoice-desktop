//! Desktop application lifecycle. Ordinary room/window orchestration stays in the shell.

use tauri::{AppHandle, Builder};

#[cfg(target_os = "macos")]
#[path = "application/macos.rs"]
mod imp;
#[cfg(not(target_os = "macos"))]
mod imp {
    pub struct Os;
    impl super::Application for Os {}
}
pub use imp::Os as NativeApplication;

pub trait Application {
    /// Install single-instance handling before launch. A second launch shows, restores and focuses the room.
    fn configure(builder: Builder<tauri::Wry>) -> Builder<tauri::Wry> {
        builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| crate::show_room(app)))
    }

    /// Prepare application policy on the UI thread, before any window exists. Tauri defaults outside macOS.
    fn prepare(_app: &AppHandle) -> tauri::Result<()> {
        Ok(())
    }

    /// Finish launch activation on the UI thread, after the shell shows and focuses the room.
    fn activate(_app: &AppHandle) {}

    /// Handle native lifecycle events on the UI thread. macOS Dock reopen shows a hidden/missing room.
    fn handle_event(_app: &AppHandle, _event: &tauri::RunEvent) {}

    /// Native application observations for diagnostics, on the UI thread. Never invent AppKit facts elsewhere.
    fn report(_app: &AppHandle) -> String {
        "application=tauri".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_lifecycle_uses_tauri_without_native_policy() {
        struct DefaultApplication;
        impl Application for DefaultApplication {}
        // The default backend accepts configuration without requiring a native desktop.
        let _builder = DefaultApplication::configure(tauri::Builder::default());
    }
}
