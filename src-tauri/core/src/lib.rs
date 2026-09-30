//! The desktop app's pure logic, kept apart from the Tauri shell so it is unit-tested on any
//! machine (the shell needs a webview toolkit to even link).

pub mod bridge;
pub mod engines;
pub mod headset;
pub mod media;
pub mod settings;
