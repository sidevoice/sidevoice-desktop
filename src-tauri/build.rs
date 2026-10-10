fn main() {
    // The app's own commands, declared so capabilities can grant them per window/origin:
    // the settings window gets the settings commands (and reads the native engine's state), the room window
    // `bridge_state` and `bridge_level` (+ `debug_log`), the native engine, the voice call (`voice_*`, macOS) and the
    // local host (`local_host_*`), the call controls card its own four.
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
        "engine_memory",
        "engine_catalogs",
        "engine_set_credential",
        "engine_has_credential",
        "voice_set_settings",
        "voice_start",
        "voice_stop",
        "voice_say",
        "voice_cancel_say",
        "voice_mute",
        "voice_cancel_input",
        "local_host_state",
        "local_host_pairing",
        "local_host_action",
        "local_host_pairing_code",
        "local_host_pair_room",
    ])))
    .expect("failed to run tauri-build");
}
