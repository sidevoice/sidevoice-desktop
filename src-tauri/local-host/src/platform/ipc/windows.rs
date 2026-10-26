//! Windows: nothing yet. The named pipe's peer and its shutdown come with the Windows local host; until then no peer
//! is trusted, so nothing is ever said over a pipe.

use super::Os;
use std::convert::Infallible;

pub struct Native;

impl Os for Native {
    type User = Infallible;
}
