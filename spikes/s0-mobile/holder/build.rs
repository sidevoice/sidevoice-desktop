// Only the app's Rust calls the holder (`run_mobile_plugin`): no command is offered to pages.
const COMMANDS: &[&str] = &[];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .ios_path("ios")
        .build();
}
