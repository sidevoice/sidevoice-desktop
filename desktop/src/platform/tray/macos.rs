use super::Os;

pub(super) struct Native;

impl Os for Native {
    const TEMPLATE_ART: bool = true;
}
