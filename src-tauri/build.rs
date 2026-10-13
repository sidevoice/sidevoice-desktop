fn main() {
    build_channel();
    // The app's own commands, declared so capabilities can grant them per window/origin:
    // the settings window gets the settings commands (and reads the native engine's state), the room window
    // `bridge_state` and `bridge_level` (+ `debug_log`), the native engine, the voice call (`voice_*`, macOS) and the
    // local host (`local_host_*`) and the app's updates (`updater`), the call controls card its own four.
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
        "engine_uninstall",
        "engine_memory",
        "engine_catalogs",
        "engine_set_credential",
        "engine_has_credential",
        "engine_test_credential",
        "engine_refresh",
        "voice_set_settings",
        "voice_start",
        "voice_stop",
        "voice_say",
        "voice_cancel_say",
        "voice_mute",
        "voice_cancel_input",
        "voice_audio_devices",
        "voice_set_audio_devices",
        "local_host_state",
        "local_host_pairing",
        "local_host_action",
        "local_host_pairing_code",
        "local_host_pair_room",
        "updater",
    ])))
    .expect("failed to run tauri-build");
}

/// What this build is (core `build_info`, `host.build`; RELEASING.md → "Channels"), from what `cargo xtask` sets:
/// `SIDEVOICE_BUILD_CHANNEL` (`release`, `nightly` or `dev`; unset or empty is `dev`, so a laptop's build is never a
/// release), `SIDEVOICE_BUILD_COMMIT` (a short commit) and `SIDEVOICE_BUILD_DATE` (`YYYY-MM-DD`), which a nightly must
/// have. Handed to the app's code under the same names; an unknown channel stops the build. A release and (for now) a
/// nightly have the updater.
fn build_channel() {
    const CHANNEL: &str = "SIDEVOICE_BUILD_CHANNEL";
    const COMMIT: &str = "SIDEVOICE_BUILD_COMMIT";
    const DATE: &str = "SIDEVOICE_BUILD_DATE";
    let var = |name: &str| {
        println!("cargo:rerun-if-env-changed={name}");
        std::env::var(name).unwrap_or_default()
    };
    let (channel, commit, date) = (var(CHANNEL), var(COMMIT), var(DATE));
    let channel = if channel.is_empty() { "dev".to_owned() } else { channel };
    assert!(matches!(channel.as_str(), "release" | "nightly" | "dev"), "{CHANNEL}={channel}: release, nightly or dev");
    assert!(channel != "nightly" || (!commit.is_empty() && !date.is_empty()), "a nightly says its commit and date");
    println!("cargo:rustc-env={CHANNEL}={channel}");
    println!("cargo:rustc-env={COMMIT}={commit}");
    println!("cargo:rustc-env={DATE}={date}");
}
