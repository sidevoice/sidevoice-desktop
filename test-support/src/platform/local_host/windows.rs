//! Windows: no local host yet, so neither mode can run.
use super::LocalHostCi;
use std::io;

pub struct Os;

fn unsupported() -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "the local host is not available on this OS yet"))
}

impl LocalHostCi for Os {
    fn serve(_data: &str, _app: &str, _port_file: &str) -> io::Result<()> {
        unsupported()
    }
    fn attack(_data: &str, _app: &str, _port: u16, _socket: &str) -> io::Result<()> {
        unsupported()
    }
}
