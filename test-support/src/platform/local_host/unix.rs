//! Unix: the local host's `serve` and `attack` modes of `local-host-ci`.
use super::LocalHostCi;
use crate::fake_core::FakeCore;
use sidevoice_local_host::host::{Config, LocalHost};
use sidevoice_local_host::paths::DataDirs;
use sidevoice_local_host::state::State;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::exit;
use std::time::Duration;

pub struct Os;

impl LocalHostCi for Os {
    fn serve(data: &str, app: &str, port_file: &str) -> io::Result<()> {
        let dirs = DataDirs::new(data);
        let _core = FakeCore::start(&dirs.data, 1, true).expect("the stand-in core starts");
        std::fs::set_permissions(&dirs.data, std::fs::Permissions::from_mode(0o755)).expect("chmod D");
        let config = Config { dirs, app_dir: PathBuf::from(app), name: "ci".into() };
        let host = LocalHost::start(config, |line| println!("{line}")).expect("the proxy starts");
        let report = host.poll();
        if report.state != State::Running {
            eprintln!("not running: {report:?}");
            exit(1);
        }
        // The first user can: the secret it holds opens the proxy.
        let secret = host.pairing().expect("a pairing")["token"].as_str().unwrap().to_string();
        let port = host.proxy().port();
        let status = status_of(
            &format!(
                "GET /api/device/devices HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: tauri://localhost\r\n\
         Authorization: Bearer {secret}\r\n\r\n"
            ),
            port,
        );
        println!("serve: the first user through the proxy with the secret -> {status}");
        if status != 200 {
            exit(1);
        }
        std::fs::write(port_file, port.to_string()).expect("port file");
        host.watch();
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    fn attack(data: &str, app: &str, port: u16, socket: &str) -> io::Result<()> {
        let dirs = DataDirs::new(data);
        let mut breaches = 0;
        let mut report = |what: &str, got_through: bool, detail: String| {
            println!("attack: {what}: {} ({detail})", if got_through { "GOT THROUGH" } else { "refused" });
            breaches += usize::from(got_through);
        };

        // The socket: pairing, the connector link and every route behind it.
        match UnixStream::connect(socket) {
            Ok(mut stream) => {
                let _ =
                    stream.write_all(b"GET /api/local/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
                let mut answer = String::new();
                let _ = stream.read_to_string(&mut answer);
                report("open the core's socket", true, answer.lines().next().unwrap_or("").to_string());
            }
            Err(e) => report("open the core's socket", false, e.to_string()),
        }
        match std::fs::read_dir(dirs.core()) {
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
}

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
