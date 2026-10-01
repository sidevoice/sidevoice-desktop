fn main() {
    // The app's own commands, declared so capabilities can grant them per window/origin:
    // the settings window gets the settings commands (and reads the native engine's state), the room window
    // `bridge_state` (+ `debug_log`) and the native engine.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
        "get_settings",
        "save_settings",
        "bridge_state",
        "debug_log",
        "headset_report",
        "headset_test",
        "engine_capabilities",
        "engine_installed",
        "engine_on_disk",
        "engine_install",
        "engine_progress",
        "engine_transcribe",
        "engine_synthesize",
    ])))
    .expect("failed to run tauri-build");
}
