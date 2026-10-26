//! Windows: no local host yet (its crate is a Unix-only dependency), so both modes answer unsupported.
use super::Modes;

pub struct Os;

impl Modes for Os {}
