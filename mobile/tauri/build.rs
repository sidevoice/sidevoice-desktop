fn main() {
    // The app's own commands, declared so its capability can grant them: the page's number and its report of the call
    // (`call_page`, `call_state`, `keep_screen_on`). What
    // the apps share is the plugin's (sidevoice-shared-tauri).
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&["call_page", "call_state", "keep_screen_on"]),
    ))
    .expect("failed to run tauri-build");
}
