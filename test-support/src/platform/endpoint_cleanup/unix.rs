use super::Endpoints;

pub struct Os;

impl Endpoints for Os {
    const LEAVE_DIRECTORIES: bool = true;
}
