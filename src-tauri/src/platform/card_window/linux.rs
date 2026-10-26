use super::CardWindow;

/// Linux uses the shared Tauri defaults: no native window adjustments are needed.
pub struct NativeCardWindow;

impl CardWindow for NativeCardWindow {}
