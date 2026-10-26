use super::{CoordinateUnit, Os};

pub(super) struct Native;
impl Os for Native {
    const UNIT: CoordinateUnit = CoordinateUnit::Points;
}
