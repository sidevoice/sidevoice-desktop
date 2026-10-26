//! Unix: a Unix domain socket. Its peer's effective uid (`SO_PEERCRED` on Linux, `LOCAL_PEERCRED` on macOS) through
//! `interprocess`, this process's from `rustix`, both without `unsafe` of ours.

use super::Os;
use interprocess::local_socket::traits::StreamCommon as _;
use interprocess::local_socket::Stream as Local;
use std::fmt;
use std::io;
use std::net::Shutdown;

/// A user as its effective uid.
#[derive(Debug, PartialEq, Eq)]
pub struct Uid(u32);

impl fmt::Display for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "uid {}", self.0)
    }
}

pub struct Native;

impl Os for Native {
    type User = Uid;
    /// The socket keeps its deadlines and its end itself.
    type Shared = ();

    fn this_user() -> io::Result<Uid> {
        Ok(Uid(rustix::process::geteuid().as_raw()))
    }

    fn peer_user(stream: &Local) -> io::Result<Uid> {
        let creds = stream.peer_creds()?;
        creds.euid().map(Uid).ok_or_else(|| io::Error::other("the socket does not say who its peer is"))
    }

    fn shutdown(stream: &Local, _shared: &()) -> io::Result<()> {
        match stream {
            Local::UdSocket(socket) => socket.inner().shutdown(Shutdown::Both),
        }
    }

    /// The socket inside `dir` itself, so it goes away with it.
    #[cfg(test)]
    fn scratch_endpoint(dir: &std::path::Path, name: &str) -> sidevoice_platform::ipc::Endpoint {
        dir.join(format!("{name}.sock")).to_str().expect("a UTF-8 scratch path").parse().expect("an absolute path")
    }
}
