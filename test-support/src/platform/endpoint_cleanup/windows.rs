//! Windows: endpoints are named pipes, which leave no directory.
use super::Endpoints;

pub struct Os;

impl Endpoints for Os {}
