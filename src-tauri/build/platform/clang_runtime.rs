//! Clang runtime linking for the Cargo compilation target.

mod macos;
mod other;

/// Links the Clang runtime required by the compilation target.
pub trait ClangRuntime {
    /// Emits the Apple runtime archive link argument for macOS; other targets emit nothing.
    fn link(&self);
}

/// Selects using Cargo's compilation target, which can differ from the host running this build script.
pub fn for_compilation_target() -> &'static dyn ClangRuntime {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        &macos::Target
    } else {
        &other::Target
    }
}
