//! Linux keeps nothing for the app beyond its folders: the trait's default.

use super::Os;

pub(super) struct Native;

impl Os for Native {}
