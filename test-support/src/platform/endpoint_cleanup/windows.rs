use super::*;

pub struct Os;
impl EndpointCleanup for Os {
    fn forget(_data: &Path) {}
}
