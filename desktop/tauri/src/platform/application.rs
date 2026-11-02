//! Desktop application lifecycle. Ordinary room/window orchestration stays in the shell.

use sidevoice_platform::fs::{Files, Fs};
use std::path::PathBuf;
use tauri::{AppHandle, Builder};

#[cfg_attr(target_os = "macos", path = "application/macos.rs")]
#[cfg_attr(target_os = "windows", path = "application/windows.rs")]
#[cfg_attr(target_os = "linux", path = "application/linux.rs")]
mod imp;
use imp::Native;

/// What differs per OS; Tauri's defaults are the rest.
trait Os {
    /// Whether the installed app is checked as a Sidevoice program before the reset's helper or relaunch starts it.
    /// Where it is not, the launch it shipped with stands: the executable as the OS names it, unchecked.
    const PROGRAM_CHECKED: bool = true;

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
    Native::prepare(app)
}

/// Finish launch activation on the UI thread, after the shell shows and focuses the room.
pub fn activate(app: &AppHandle) {
    Native::activate(app)
}

/// Handle native lifecycle events on the UI thread. macOS Dock reopen shows a hidden/missing room.
pub fn handle_event(app: &AppHandle, event: &tauri::RunEvent) {
    Native::handle_event(app, event)
}

/// Native application observations for diagnostics, on the UI thread. Never invent AppKit facts elsewhere.
pub fn report(app: &AppHandle) -> String {
    Native::report(app).unwrap_or_else(|| "application=tauri".into())
}

/// The installed app, as Tauri resolves its own binary (Linux: the AppImage itself rather than its temporary mount,
/// which goes away with the app; Tauri takes it from `APPIMAGE`, an environment the app trusts as it trusts `HOME`),
/// checked as a Sidevoice program (`trusted_program`: this user's or the system's, and nobody else may change it or the
/// folders above it). Where the shipped launch is kept (`PROGRAM_CHECKED` false: macOS), the executable as the OS
/// names it. The reset's helper and relaunch start only this; neither the page nor the plan names a program.
pub fn installed_program() -> Option<PathBuf> {
    if !Native::PROGRAM_CHECKED {
        return std::env::current_exe().ok();
    }
    let program = tauri::process::current_binary(&tauri::Env::default()).ok()?;
    Files::trusted_program(&program).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_needs_no_native_desktop() {
        let _builder = configure(tauri::Builder::default());
    }
}
