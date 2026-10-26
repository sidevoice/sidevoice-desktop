//! The only module of xtask that touches the operating system; one file per concern.

pub mod file_mode;
pub mod host;
pub mod lifecycle;
pub mod macos_app;
pub mod private_files;
pub mod probe;
pub mod release;
pub mod two_users;
