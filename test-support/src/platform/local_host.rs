//! The local host's two CI-only modes of `local-host-ci`: `serve` as the first user, `attack` as a second one.
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::Duration;

#[cfg(unix)]
#[path = "local_host/unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "local_host/windows.rs"]
mod imp;

use imp::Os;

/// A connection to the core's endpoint.
trait Duplex: Read + Write {}
impl<T: Read + Write> Duplex for T {}

/// The only parts of the two modes that differ per OS. A host that does not run the scenario answers
/// [`io::ErrorKind::Unsupported`]: today, every host but Unix (the two-user CI scenario is Unix's).
trait Modes {
    /// [`serve`], whole: it runs the local host itself, with `D` opened to a second user the Unix way (its mode bits);
    /// the two-user scenario is a Unix CI job.
    fn serve(_data: &Path, _app: &Path, _port_file: &Path) -> io::Result<()> {
        Err(unsupported())
    }

    /// Opens the core's endpoint at `socket` raw, without the shared channel's peer check, so that what is measured is
    /// whether the OS lets this user in at all.
    fn open_core_socket(_socket: &Path) -> io::Result<Box<dyn Duplex>> {
        Err(unsupported())
    }
}

fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "the local host is not available on this OS yet")
}

/// As the first user: the stand-in core and the app's side paired with it, the proxy on loopback. Writes the proxy's
/// port (only the port) to `port_file` and serves until killed. Leaves `data` open for a second user to meet the core
/// directory's own refusal.
pub fn serve(data: &Path, app: &Path, port_file: &Path) -> io::Result<()> {
    Os::serve(data, app, port_file)
}

/// As a second user: must fail to open the core's `socket`, to list the core's directory, to read the app's pairing and
/// to use the proxy at `port` without the secret. Prints one line per attempt; the process exits 1 if any of them got
/// through.
pub fn attack(data: &Path, app: &Path, port: u16, socket: &Path) -> io::Result<()> {
    let mut breaches = 0;
    let mut report = |what: &str, got_through: bool, detail: String| {
        println!("attack: {what}: {} ({detail})", if got_through { "GOT THROUGH" } else { "refused" });
        breaches += usize::from(got_through);
    };

    // The socket: pairing, the connector link and every route behind it.
    match Os::open_core_socket(socket) {
        Ok(mut stream) => {
            let _ = stream.write_all(b"GET /api/local/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
            let mut answer = String::new();
            let _ = stream.read_to_string(&mut answer);
            report("open the core's socket", true, answer.lines().next().unwrap_or("").to_string());
        }
        Err(e) if e.kind() == io::ErrorKind::Unsupported => return Err(e),
        Err(e) => report("open the core's socket", false, e.to_string()),
    }
    match std::fs::read_dir(sidevoice_platform::data_dir::core_dir(data)) {
        Ok(_) => report("list the core's directory", true, String::new()),
        Err(e) => report("list the core's directory", false, e.to_string()),
    }
    match std::fs::read(PathBuf::from(app).join("local-host.json")) {
        Ok(_) => report("read the app's local-host.json", true, String::new()),
        Err(e) => report("read the app's local-host.json", false, e.to_string()),
    }

    // The proxy, with everything right but the secret it cannot know.
    let host = format!("127.0.0.1:{port}");
    for (what, path, auth) in [
        ("proxy without the secret", "/api/device/devices", ""),
        (
            "proxy with a guessed secret",
            "/api/device/devices",
            "Authorization: Bearer AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\r\n",
        ),
        ("the connector link through the proxy", "/api/connectors/link/?EIO=4&transport=polling", ""),
        ("local pairing through the proxy", "/api/device/local/pair", ""),
    ] {
        let status = status_of(&format!("POST {path} HTTP/1.1\r\nHost: {host}\r\nOrigin: tauri://localhost\r\n{auth}Content-Length: 2\r\n\r\n{{}}"), port);
        report(what, (200..300).contains(&status), format!("HTTP {status}"));
    }
    let status = status_of(
        &format!(
            "GET /api/browser/call HTTP/1.1\r\nHost: {host}\r\nOrigin: tauri://localhost\r\nUpgrade: websocket\r\n\
     Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\
     Sec-WebSocket-Protocol: sidevoice, sidevoice.token.guess\r\n\r\n"
        ),
        port,
    );
    report("a WebSocket through the proxy without the secret", status == 101, format!("HTTP {status}"));

    if breaches > 0 {
        println!("attack: {breaches} attempt(s) got through");
        exit(1);
    }
    Ok(())
}

/// The HTTP status the proxy on loopback `port` answers `raw` with; 0 when it answers nothing readable.
fn status_of(raw: &str, port: u16) -> u16 {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return 0;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.write_all(raw.as_bytes());
    let mut answer = Vec::new();
    let _ = stream.read_to_end(&mut answer);
    String::from_utf8_lossy(&answer).split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0)
}
