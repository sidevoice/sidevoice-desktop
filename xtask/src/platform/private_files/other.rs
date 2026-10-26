//! Hosts without a creation mask: nothing to hold.

pub struct Held;

impl Held {
    pub fn start() -> Held {
        Held
    }
}
