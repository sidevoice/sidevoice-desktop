//! What this process is, for the start event of the log (the logging guideline, section 5.1): the app's name and version
//! and the operating system and CPU as Rust names them, which the shared logging turns into OpenTelemetry's values. A value-only
//! difference, so a plain module: no trait, no per-OS files.

use sidevoice_shared::logging::Resource;

pub fn current() -> Resource {
    Resource::of("sidevoice-desktop", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH)
}
