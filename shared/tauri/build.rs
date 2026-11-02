include!("commands.rs");

#[path = "build/platform.rs"]
mod platform;

fn main() {
    // `android/` is the library an Android app's project takes from this plugin: the Kotlin class that asks the person for
    // the microphone (platform/microphone/android.rs). Building for another OS ignores it.
    tauri_plugin::Builder::new(COMMANDS).android_path("android").build();
    // Its unit-test binaries link Tauri, and on Windows would not start without the manifest.
    platform::app_manifest::link_into_every_executable();
}
