//! Tauri's defaults already provide the application lifecycle on this host.

use super::Os;

pub(super) struct Native;

impl Os for Native {}
