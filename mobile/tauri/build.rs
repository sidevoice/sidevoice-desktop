#![allow(clippy::disallowed_macros, reason = "a build script talks to cargo and the terminal")]
fn main() {
    build_channel();
    // The app's own commands, declared so its capability can grant them: the page's number, its report of the call, the
    // creatures' stills its surfaces show (`call_page`, `call_state`, `call_still`, `call_forget`) and the developer
    // options' one (`developer_options`), the headset's presses' (`global_shortcuts`), the cues' player (`cue`), the system's output picker (`audio_route`) and whether the
    // screen is locked (`screen_locked`). What the apps share is the plugin's (sidevoice-shared-tauri).
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
        "audio_route",
        "call_page",
        "call_state",
        "call_still",
        "call_forget",
        "cue",
        "developer_options",
        "global_shortcuts",
        "screen_locked",
    ])))
    .expect("failed to run tauri-build");
}

/// What this build is (`host.build`, src/bridge.rs `build`; RELEASING.md -> "Channels"), from what `cargo xtask mobile`
/// sets, as the desktop's build script reads it (desktop/tauri/build.rs): `SIDEVOICE_BUILD_CHANNEL` (`release`,
/// `nightly` or `dev`; unset or empty is `dev`, so a laptop's build is never a release), `SIDEVOICE_BUILD_COMMIT` (a short
/// commit) and `SIDEVOICE_BUILD_DATE` (`YYYY-MM-DD`), which a nightly must have. Handed to the app's code under the same
/// names; an unknown channel stops the build.
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
