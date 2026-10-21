//! The desktop app's pure logic, kept apart from the Tauri shell so it is unit-tested on any
//! machine (the shell needs a webview toolkit to even link).

pub mod bridge;
pub mod build_info;
pub mod call_controls;
pub mod computer_name;
pub mod debug_options;
pub mod headset;
pub mod i18n;
pub mod logging;
pub mod media;
pub mod memory;
pub mod settings;
pub mod updater;
pub mod voice;
