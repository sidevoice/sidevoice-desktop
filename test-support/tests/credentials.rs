// Run the shell interface's own tests without linking Tauri or the model runtimes.
#[path = "../../src-tauri/src/platform/credentials.rs"]
mod credentials;

// `clear_all` is the app's reset step; its only fixture is Linux's absent store, since native fixtures never touch the
// real namespace. Referenced here so every host compiles it without a dead-code warning.
const _: fn() -> Result<(), credentials::Error> = credentials::clear_all;
