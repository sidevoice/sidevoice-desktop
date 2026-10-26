//! The application manifest: what an executable declares to the system loader before it runs.

/// Links the application manifest into every executable this package builds, the unit-test binaries included.
///
/// MSVC targets (Windows): packaging/windows/app.manifest, tauri-build's default (Common Controls v6, whose comctl32
/// entry points tao's dialogs import), through the linker (`/MANIFEST:EMBED /MANIFESTINPUT`). tauri-build links its own
/// into the package's binaries only, and a test binary without one fails to start (STATUS_ENTRYPOINT_NOT_FOUND,
/// tauri-apps/tauri#13419), so the build script turns tauri-build's off and this is the app's only manifest, as Tauri
/// does for its own tests. Every other target: nothing, as no other loader reads such a manifest.
pub fn link_into_every_executable() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
        .join("packaging/windows/app.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
}
