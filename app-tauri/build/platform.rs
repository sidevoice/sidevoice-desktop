//! What the build script does differently per target operating system. A build script runs on the build host, so each
//! concern selects by the target Cargo builds for (`CARGO_CFG_TARGET_*`), never by `cfg`, and cross builds stay right.

#[path = "platform/app_manifest.rs"]
pub mod app_manifest;
