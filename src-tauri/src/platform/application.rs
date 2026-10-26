//! Desktop application lifecycle. Ordinary room/window orchestration stays in the shell.

use tauri::{AppHandle, Builder};

#[cfg_attr(target_os = "macos", path = "application/macos.rs")]
#[cfg_attr(target_os = "windows", path = "application/windows.rs")]
#[cfg_attr(target_os = "linux", path = "application/linux.rs")]
mod imp;
use imp::Os;

/// What differs per OS; Tauri's defaults are the rest.
trait Native {
    /// Application policy before any window exists. Tauri's defaults need none.
    fn prepare(_app: &AppHandle) -> tauri::Result<()> {
        Ok(())
    }

    /// Native activation after the room is shown and focused. Tauri's focus is enough by default.
    fn activate(_app: &AppHandle) {}

    /// Native lifecycle events Tauri reports only on this OS.
    fn handle_event(_app: &AppHandle, _event: &tauri::RunEvent) {}

    /// Native application observations; `None` where the OS reports nothing beyond Tauri.
    fn report(_app: &AppHandle) -> Option<String> {
        None
    }
}

/// Install single-instance handling before launch. A second launch shows, restores and focuses the room.
pub fn configure(builder: Builder<tauri::Wry>) -> Builder<tauri::Wry> {
    builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| crate::show_room(app)))
}

/// Prepare application policy on the UI thread, before any window exists (macOS: a regular Dock app).
pub fn prepare(app: &AppHandle) -> tauri::Result<()> {
    Os::prepare(app)
}

/// Finish launch activation on the UI thread, after the shell shows and focuses the room.
pub fn activate(app: &AppHandle) {
    Os::activate(app)
}

/// Handle native lifecycle events on the UI thread. macOS Dock reopen shows a hidden/missing room.
pub fn handle_event(app: &AppHandle, event: &tauri::RunEvent) {
    Os::handle_event(app, event)
}

/// Native application observations for diagnostics, on the UI thread. Never invent AppKit facts elsewhere.
pub fn report(app: &AppHandle) -> String {
    Os::report(app).unwrap_or_else(|| "application=tauri".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_needs_no_native_desktop() {
        let _builder = configure(tauri::Builder::default());
    }
}
