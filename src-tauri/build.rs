fn main() {
    // The app's own commands, declared so capabilities can grant them per window/origin:
    // the settings window gets `settings`, the room page gets only `bridge_state`.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
        "get_settings",
        "save_settings",
        "bridge_state",
    ])))
    .expect("failed to run tauri-build");
}
