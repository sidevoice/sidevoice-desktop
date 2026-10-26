use super::{CoordinateUnit, Native};

pub(super) struct Os;
impl Native for Os {
    const UNIT: CoordinateUnit = CoordinateUnit::Points;
}
