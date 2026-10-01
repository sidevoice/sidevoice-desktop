//! The local host against the real sidevoice-core, when one is at hand: `SIDEVOICE_CORE_PYTHON` names a Python that
//! imports `sidevoice_core` (a checkout's `.venv/bin/python`). Skipped without it — CI here has no core; the stand-in
//! (`fake_core`) covers the same ground in `tests/local_host.rs`.
//!
//! The core is started as its supervisor would (`--data-dir C --port 0 --idle-exit 0`, `C` 0700), and the app's side
//! pairs with it over `C/local.sock`, proves its identity, and carries a page's request through the proxy.

use serde_json::Value;
use sidevoice_local_host::core_socket::CoreSocket;
use sidevoice_local_host::host::{Config, LocalHost};
use sidevoice_local_host::paths::DataDirs;
use sidevoice_local_host::state::State;
use sidevoice_local_host::store;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::DirBuilderExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Core(Child);

impl Drop for Core {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn get(port: u16, path: &str, secret: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let raw = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: tauri://localhost\r\nAuthorization: Bearer {secret}\r\n\r\n"
    );
    stream.write_all(raw.as_bytes()).unwrap();
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    let status = answer.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    (status, answer.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default())
}

#[test]
fn pairs_with_the_real_core_and_carries_a_page_request() {
    let Some(python) = std::env::var_os("SIDEVOICE_CORE_PYTHON") else {
        eprintln!("skipped: SIDEVOICE_CORE_PYTHON names no Python with sidevoice_core");
        return;
    };
    let tmp = tempfile::Builder::new().prefix("svrc").tempdir_in("/tmp").unwrap();
    let dirs = DataDirs::new(tmp.path().join("d"));
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dirs.core()).unwrap();
    let _core = Core(
        Command::new(python)
            .args(["-m", "sidevoice_core.server", "--port", "0", "--idle-exit", "0", "--data-dir"])
            .arg(dirs.core())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let socket = CoreSocket::new(&dirs);
    let started = Instant::now();
    while socket.health().is_err() {
        assert!(started.elapsed() < Duration::from_secs(60), "the core never answered on its socket");
        std::thread::sleep(Duration::from_millis(200));
    }

    let app_dir = tmp.path().join("app");
    let config = Config { dirs: dirs.clone(), app_dir: app_dir.clone(), name: "real-core-test".into() };
    let host = LocalHost::start(config.clone(), |_| {}).unwrap();
    assert_eq!(host.poll().state, State::Running, "{:?}", host.state());
    let pairing = host.pairing().unwrap();
    let secret = pairing["token"].as_str().unwrap().to_string();
    let port = host.proxy().port();
    let first = store::load(&app_dir).unwrap();

    // A page's authenticated request, through the proxy: the core answers with the device token's view.
    let (status, body) = get(port, "/api/device/devices", &secret);
    assert_eq!(status, 200, "{body}");
    let devices: Value = serde_json::from_str(&body).unwrap();
    let rows = devices["devices"].as_array().unwrap();
    assert!(rows.iter().any(|d| d["kind"] == "local"), "{devices}");
    // Native's own routes stay native's, secret or not.
    assert_eq!(get(port, "/api/local/health", &secret).0, 404);
    assert_eq!(get(port, "/api/device/devices", "wrong").0, 401);

    // The app again: the stored pairing is proven and reused, no new device.
    host.shutdown();
    let again = LocalHost::start(config, |_| {}).unwrap();
    assert_eq!(again.poll().state, State::Running);
    assert_eq!(store::load(&app_dir).unwrap().device_id, first.device_id);

    // «Volver a conectar»: a new local device, and the core refuses the old token.
    assert_eq!(again.reconnect().state, State::Running);
    let second = store::load(&app_dir).unwrap();
    assert_ne!(second.device_id, first.device_id);
    assert_eq!(socket.accepts(&first.token), Ok(false));
    assert_eq!(socket.accepts(&second.token), Ok(true));
}
