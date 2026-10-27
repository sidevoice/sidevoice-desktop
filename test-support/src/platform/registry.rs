//! Registry transport for fixture archives and its native installer adapter.
use crate::fake_registry::FakeRegistry;
use sidevoice_local_host::{install::Failure, npm::Fetch};
use std::io::{self, Read};
use std::time::Duration;

/// A response supplied by the in-memory registry, including deliberately malformed lengths.
pub struct Response {
    pub status: u16,
    pub length: Option<u64>,
    pub body: Box<dyn Read + Send>,
}

/// Retrieve one registry URL, recording the request and preserving slow or malformed responses.
pub trait RegistryTransport {
    fn get(&self, url: &str) -> Response;
}

struct Slow {
    body: io::Cursor<Vec<u8>>,
    delay: Duration,
}

impl Read for Slow {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        std::thread::sleep(self.delay);
        let n = buf.len().min(1024);
        self.body.read(&mut buf[..n])
    }
}

impl RegistryTransport for FakeRegistry {
    fn get(&self, url: &str) -> Response {
        self.asked.lock().unwrap().push(url.to_string());
        let Some(served) = self.served.lock().unwrap().get(url).cloned() else {
            return Response { status: 404, length: None, body: Box::new(io::empty()) };
        };
        let body: Box<dyn Read + Send> = match served.delay {
            Some(delay) => Box::new(Slow { body: io::Cursor::new(served.body), delay }),
            None => Box::new(io::Cursor::new(served.body)),
        };
        Response { status: served.status, length: served.length, body }
    }
}

impl Fetch for FakeRegistry {
    fn get(&self, url: &str) -> Result<sidevoice_local_host::npm::Response, Failure> {
        let Response { status, length, body } = RegistryTransport::get(self, url);
        Ok(sidevoice_local_host::npm::Response { status, length, body })
    }
}
