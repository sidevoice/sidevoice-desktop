//! The modules of the build tooling (`cargo xtask`, main.rs): a library so that a step one host has no use for is
//! not dead code on it.

pub mod checks;
pub mod host_graph;
pub mod packaging;
pub mod platform;
pub mod platform_guard;
pub mod publish;
pub mod util;

pub type Result<T> = std::result::Result<T, String>;
