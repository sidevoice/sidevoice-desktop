//! Nothing beyond the portable observations is available here: no exit destructors to observe, no footprint, Metal or
//! accelerator reading, no call of its own held by the app (the page runs it), and no WebView2 diagnostic binding.

use super::Os;

pub(super) struct Native;

impl Os for Native {}
