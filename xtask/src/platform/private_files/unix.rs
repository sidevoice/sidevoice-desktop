//! Unix: the process creation mask.

pub struct Held {
    before: rustix::fs::Mode,
}

impl Held {
    pub fn start() -> Held {
        Held { before: rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077)) }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        rustix::process::umask(self.before);
    }
}
