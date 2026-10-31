include!("commands.rs");

#[path = "build/platform.rs"]
mod platform;

fn main() {
    tauri_plugin::Builder::new(COMMANDS).build();
    // Its unit-test binaries link Tauri, and on Windows would not start without the manifest.
    platform::app_manifest::link_into_every_executable();
}
