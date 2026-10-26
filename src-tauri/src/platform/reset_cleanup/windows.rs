//! No reset cleanup here yet (`reset_availability`): the trait's unavailable defaults.

use super::ResetCleanup;

pub struct NativeResetCleanup;

impl ResetCleanup for NativeResetCleanup {}
