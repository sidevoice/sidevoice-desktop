//! Windows: the two-user CI scenario is a Unix job, so both modes answer unsupported.
use super::Modes;

pub struct Os;

impl Modes for Os {}
