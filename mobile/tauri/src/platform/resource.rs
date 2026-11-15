//! What this process is, for the crash reports (the same closed list as the log's start event): the app's name and version and
//! the operating system and CPU as Rust names them, which the shared logging turns into OpenTelemetry's values. A value-only
//! difference, so a plain module: no trait, no per-OS files.

use sidevoice_shared::logging::Resource;

pub fn current() -> Resource {
    Resource::of(
        "sidevoice-mobile",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}
