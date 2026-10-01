fn main() {
    // The app's own commands, declared so capabilities can grant them per window/origin:
    // the settings window gets the settings commands (and reads the native engine's state), the room window
    // `bridge_state` and `bridge_level` (+ `debug_log`) and the native engine, the call controls card its own four.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
        "get_settings",
        "save_settings",
        "bridge_state",
        "bridge_level",
        "call_controls_ready",
        "call_controls_run",
        "call_controls_layout",
        "call_controls_drag",
        "debug_log",
        "headset_report",
        "headset_test",
        "engine_capabilities",
        "engine_installed",
        "engine_on_disk",
        "engine_install",
        "engine_progress",
        "engine_cancel",
        "engine_transcribe",
        "engine_synthesize",
        "engine_load",
        "engine_unload",
        "engine_loaded",
        "engine_memory",
    ])))
    .expect("failed to run tauri-build");
}
