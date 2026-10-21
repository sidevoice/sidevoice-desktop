fn main() {
    build_channel();
    link_macos_clang_runtime();
    // The app's own commands, declared so capabilities can grant them per window/origin:
    // the room window `bridge_state`, `bridge_level` and `bridge_playback` (+ `debug_log`), the native engine, the
    // voice call (`voice_*`, macOS) and the local host (`local_host_*`), the mute shortcut and the call controls settings (`mute_shortcut`,
    // `call_controls_settings`), the app's updates (`updater`), the microphone permission (`microphone`, macOS) and the
    // developer options (`debug_options`), the call controls card its own five.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
        "bridge_state",
        "bridge_level",
        "bridge_playback",
        "call_controls_ready",
        "call_controls_run",
        "call_controls_layout",
        "call_controls_drag",
        "call_controls_resize",
        "debug_log",
        "engine_capabilities",
        "engine_installed",
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
        "voice_create_call",
        "voice_route_input",
        "voice_dispose_call",
        "voice_ready",
        "voice_update",
        "voice_preload",
        "voice_start",
        "voice_stop",
        "voice_say",
        "voice_cancel_say",
        "voice_mute",
        "voice_cancel_input",
        "voice_end_input",
        "voice_audio_devices",
        "voice_set_audio_devices",
        "local_host_state",
        "local_host_pairing",
        "local_host_action",
        "local_host_pairing_code",
        "local_host_pair_room",
        "local_host_install",
        "local_host_install_progress",
        "local_host_install_cancel",
        "data_pairings_read",
        "data_pairings_write",
        "data_delete_all",
        "data_uninstall",
        "data_reset_report",
        "mute_shortcut",
        "call_controls_settings",
        "updater",
        "microphone",
        "debug_options",
    ])))
    .expect("failed to run tauri-build");
}

fn link_macos_clang_runtime() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    // Rust links with -nodefaultlibs, while Whisper's Metal objects use this Clang runtime symbol.
    let output = std::process::Command::new("xcrun")
        .args(["clang", "-print-resource-dir"])
        .output()
        .expect("failed to locate the macOS Clang runtime");
    assert!(output.status.success(), "xcrun clang -print-resource-dir failed");
    let resource_dir = String::from_utf8(output.stdout).expect("invalid Clang resource directory");
    let archive = std::path::Path::new(resource_dir.trim()).join("lib/darwin/libclang_rt.osx.a");
    assert!(archive.is_file(), "missing Clang runtime: {}", archive.display());
    println!("cargo:rustc-link-arg={}", archive.display());
}

/// What this build is (core `build_info`, `host.build`; RELEASING.md → "Channels"), from what `cargo xtask` sets:
/// `SIDEVOICE_BUILD_CHANNEL` (`release`, `nightly` or `dev`; unset or empty is `dev`, so a laptop's build is never a
/// release), `SIDEVOICE_BUILD_COMMIT` (a short commit) and `SIDEVOICE_BUILD_DATE` (`YYYY-MM-DD`), which a nightly must
/// have. Handed to the app's code under the same names; an unknown channel stops the build. A release and a nightly
/// have the updater, each on its own channel's feed (core `updater::feed`).
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
    // The developer options' native code (src/debug_options.rs: WebKit's private debug preferences) is compiled only
    // into a nightly or a dev build; a release has none of it (core `debug_options::offered`).
    println!("cargo:rustc-check-cfg=cfg(sidevoice_debug_options)");
    if channel != "release" {
        println!("cargo:rustc-cfg=sidevoice_debug_options");
    }
}
