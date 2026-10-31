//! What every Sidevoice app shares and no shell is needed for: the voice call (the host that runs it, the models it
//! takes from the engine, the settings the page sends), and the app's side of sidevoice-engine. No Tauri, no window
//! and no webview: it is tested anywhere, and the apps put a shell on it.

pub mod debug;
pub mod engine;
pub mod platform;
pub mod voice;

/// The voice crate itself, for what the app handles of it (the audio devices): its pin is this crate's alone.
pub use sidevoice_voice;
