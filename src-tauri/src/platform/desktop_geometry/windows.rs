use super::{Native, Space};

pub(super) struct Os;
impl Native for Os {
    const SPACE: Space = Space::Pixels;
}
