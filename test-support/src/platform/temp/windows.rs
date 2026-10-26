//! The temporary directory's own paths are short enough, and there is neither a sticky-directory counterpart nor
//! group-only access through this API.
use super::Area;

pub struct Os;

impl Area for Os {}
