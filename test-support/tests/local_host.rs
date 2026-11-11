//! The local host end to end on this machine: a stand-in core on the real local channel (HTTP and a WebSocket echo),
//! the app's pairing with it and the startup cases (design §4.1), the proxy a page talks to — forwarding with the token
//! swapped in, a socket spliced both ways, the refusals — and the service's state and actions through a stand-in
//! connector and CLI.

use serde_json::{json, Value};
use sidevoice_desktop_test_support::fake_core::{self, FakeCore};
use sidevoice_desktop_test_support::platform::endpoint_cleanup;
use sidevoice_desktop_test_support::platform::storage::{StorageFixture, StorageFixtures};
use sidevoice_desktop_test_support::platform::temp;
use sidevoice_desktop_test_support::{fake_registry::FakeRegistry, fixture};
use sidevoice_local_host::host::{Action, Config, LocalHost};
use sidevoice_local_host::http;
use sidevoice_local_host::install;
use sidevoice_local_host::npm::Minor;
use sidevoice_local_host::paths::DataDirs;
use sidevoice_local_host::state::State;
use sidevoice_local_host::store;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct World {
    _tmp: tempfile::TempDir,
    dirs: DataDirs,
    app_dir: PathBuf,
    log: Arc<Mutex<Vec<String>>>,
}

impl Drop for World {
    fn drop(&mut self) {
        endpoint_cleanup::forget(&self.dirs.data);
    }
}

impl World {
    fn new() -> World {
        // Under /tmp: a socket's path must stay short.
        let tmp = tempfile::Builder::new().prefix("svlh").tempdir_in(temp::short_root()).unwrap();
        let dirs = DataDirs::new(tmp.path().join("d"));
        let app_dir = tmp.path().join("app");
        World { _tmp: tmp, dirs, app_dir, log: Arc::default() }
    }

    fn core(&self, seed: u8) -> Arc<FakeCore> {
        FakeCore::start(&self.dirs.data, seed, false).unwrap()
    }

    fn host(&self) -> Arc<LocalHost> {
        let config = Config {
            dirs: self.dirs.clone(),
            app_dir: self.app_dir.clone(),
            name: "test-mac".into(),
            pinned_minor: None,
        };
        let log = self.log.clone();
        LocalHost::start(config, move |line| log.lock().unwrap().push(line.to_string())).unwrap()
    }
}

/// One raw request to the proxy; the whole answer (the proxy closes after it).
fn send(port: u16, raw: &str) -> (u16, String, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.write_all(raw.as_bytes()).unwrap();
    let mut answer = Vec::new();
    let _ = stream.read_to_end(&mut answer);
    let text = String::from_utf8_lossy(&answer).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    (status, head.to_ascii_lowercase(), body.to_string())
}

fn request(port: u16, method: &str, path: &str, fields: &[(&str, &str)], body: &str) -> (u16, String, String) {
    let mut raw = format!("{method} {path} HTTP/1.1\r\n");
    for (name, value) in fields {
        raw.push_str(&format!("{name}: {value}\r\n"));
    }
    if !body.is_empty() {
        raw.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    raw.push_str("\r\n");
    raw.push_str(body);
    send(port, &raw)
}

fn running(host: &LocalHost) -> (u16, String) {
    let report = host.poll();
    assert_eq!(report.state, State::Running, "{report:?}");
    let pairing = host.pairing().expect("a pairing while running");
    (host.proxy().port(), pairing["token"].as_str().unwrap().to_string())
}

#[test]
fn pairs_over_the_socket_and_the_page_gets_only_the_secret() {
    let world = World::new();
    let core = world.core(1);
    let host = world.host();
    let (port, secret) = running(&host);

    let stored = store::load(&world.app_dir).expect("local-host.json");
    StorageFixtures::assert_private(&world.app_dir.join(store::FILE_NAME));
    assert_eq!(stored.fp, core.fingerprint());
    assert_eq!(core.local_device().as_deref(), Some(stored.device_id.as_str()));
    assert_eq!(
        host.pairing().unwrap(),
        json!({
            "fp": core.fingerprint(), "public_key": core.public_key(), "device_id": stored.device_id,
            "token": secret, "urls": [format!("http://127.0.0.1:{port}")], "host": "fake-machine",
            "local": true,
        })
    );
    assert_ne!(secret, stored.token, "the page never holds the device token");
    assert_eq!(secret.len(), 43, "32 bytes, base64url");

    let state = serde_json::to_value(host.state()).unwrap();
    assert_eq!(state["state"], "running");
    assert_eq!(state["core"]["api"], 1);
    assert_eq!(state["calls"], 0);
    // Another poll changes nothing and pairs nothing new.
    running(&host);
    assert_eq!(core.local_device().as_deref(), Some(stored.device_id.as_str()));
}

#[test]
fn the_proxy_forwards_with_the_token_and_answers_preflights_itself() {
    let world = World::new();
    let core = world.core(1);
    let host = world.host();
    let (port, secret) = running(&host);
    let host_field = format!("127.0.0.1:{port}");
    let bearer = format!("Bearer {secret}");
    let seen_before = core.requests().len();

    let (status, head, _) = request(
        port,
        "OPTIONS",
        "/api/presentation/echo",
        &[
            ("Host", &host_field),
            ("Origin", "tauri://localhost"),
            ("Access-Control-Request-Method", "POST"),
            ("Access-Control-Request-Headers", "authorization, content-type"),
            ("Access-Control-Request-Private-Network", "true"),
        ],
        "",
    );
    assert_eq!(status, 204);
    assert!(head.contains("access-control-allow-origin: tauri://localhost"), "{head}");
    assert!(head.contains("access-control-allow-private-network: true"), "{head}");
    assert_eq!(core.requests().len(), seen_before, "a preflight never reaches the core");

    let (status, head, body) = request(
        port,
        "POST",
        "/api/presentation/echo?x=1",
        &[
            ("Host", &host_field),
            ("Origin", "tauri://localhost"),
            ("Authorization", &bearer),
            ("Connection", "keep-alive"),
        ],
        r#"{"hello":"core"}"#,
    );
    assert_eq!(status, 200, "{head}");
    assert!(head.contains("connection: close"), "one request per connection: {head}");
    assert!(head.contains("access-control-allow-origin: tauri://localhost"), "the core's CORS comes back: {head}");
    let echoed: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(echoed["auth"], "device", "the secret became the device token");
    assert_eq!(echoed["origin"], "tauri://localhost", "Origin forwarded unchanged");
    assert_eq!(echoed["body"], r#"{"hello":"core"}"#);
    assert_eq!(echoed["path"], "/api/presentation/echo");
}

#[test]
fn the_proxy_refuses_what_the_table_refuses_and_the_core_never_sees_it() {
    let world = World::new();
    let core = world.core(1);
    let host = world.host();
    let (port, secret) = running(&host);
    let host_field = format!("127.0.0.1:{port}");
    let bearer = format!("Bearer {secret}");
    let seen_before = core.requests().len();
    let get = |fields: &[(&str, &str)], path: &str| request(port, "GET", path, fields, "").0;

    assert_eq!(get(&[("Host", &host_field), ("Authorization", &bearer)], "/api/x"), 403, "no Origin");
    assert_eq!(get(&[("Host", &host_field), ("Origin", ""), ("Authorization", &bearer)], "/api/x"), 403, "empty");
    assert_eq!(get(&[("Host", &host_field), ("Origin", "null"), ("Authorization", &bearer)], "/api/x"), 403, "null");
    let hostile = [("Host", host_field.as_str()), ("Origin", "https://evil.example"), ("Authorization", &bearer)];
    assert_eq!(get(&hostile, "/api/x"), 403, "hostile");
    let wrong_host = [("Host", "localhost"), ("Origin", "tauri://localhost"), ("Authorization", bearer.as_str())];
    assert_eq!(get(&wrong_host, "/api/x"), 421);
    let (status, head, _) =
        request(port, "GET", "/api/x", &[("Host", &host_field), ("Origin", "tauri://localhost")], "");
    assert_eq!(status, 401, "no secret");
    assert!(head.contains("access-control-allow-origin: tauri://localhost"), "a 401 the page can read: {head}");
    let app = [("Host", host_field.as_str()), ("Origin", "tauri://localhost"), ("Authorization", bearer.as_str())];
    for path in ["/api/local/health", "/api/device/local/pair", "/api/connectors/link/?EIO=4", "/api/%6cocal/health"] {
        assert_eq!(get(&app, path), 404, "{path}");
    }
    let twice = [app[0], app[1], app[1], app[2]];
    assert_eq!(get(&twice, "/api/x"), 403, "two Origins");
    assert_eq!(send(port, "GET /api/x HTTP/1.1\r\nHost: a\nOrigin: b\r\n\r\n").0, 400, "malformed head");
    // Refused with a body still on its way: the page reads the 401, not a reset connection.
    let big = "x".repeat(300_000);
    let (status, head, _) =
        request(port, "POST", "/api/x", &[("Host", &host_field), ("Origin", "tauri://localhost")], &big);
    assert_eq!(status, 401, "{head}");
    assert_eq!(core.requests().len(), seen_before, "{:?}", core.requests());
}

fn ws_handshake(port: u16, protocols: &str, origin: &str) -> (TcpStream, u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let raw = format!(
        "GET /api/browser/call HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {origin}\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Protocol: {protocols}\r\n\r\n"
    );
    stream.write_all(raw.as_bytes()).unwrap();
    let mut buf = Vec::new();
    let end = http::read_head(&mut stream, &mut buf).unwrap();
    let head = http::parse_response(&buf[..end]).unwrap();
    let protocol = head.headers.first("sec-websocket-protocol").unwrap_or("").to_string();
    (stream, head.status, protocol)
}

#[test]
fn a_websocket_is_spliced_both_ways_and_closed_by_a_new_pairing() {
    let world = World::new();
    let core = world.core(1);
    let host = world.host();
    let (port, secret) = running(&host);

    let (mut socket, status, protocol) =
        ws_handshake(port, &format!("sidevoice, sidevoice.token.{secret}"), "tauri://localhost");
    assert_eq!(status, 101);
    assert_eq!(protocol, "sidevoice");
    assert!(core.requests().iter().any(|r| r.starts_with("GET /api/browser/call") && r.ends_with("auth=device")));
    for message in ["hola", &"x".repeat(70_000)] {
        fake_core::write_frame_masked(&mut socket, 1, message.as_bytes(), Some([1, 2, 3, 4])).unwrap();
        let (opcode, payload) = fake_core::read_frame(&mut socket).unwrap();
        assert_eq!((opcode, payload.as_slice()), (1, message.as_bytes()));
    }
    assert_eq!(host.poll().calls, Some(1), "the open socket counts as a call");

    // A refused secret, a hostile origin: no socket.
    assert_eq!(ws_handshake(port, "sidevoice, sidevoice.token.wrong", "tauri://localhost").1, 401);
    assert_eq!(ws_handshake(port, "sidevoice", "tauri://localhost").1, 401);
    assert_eq!(ws_handshake(port, &format!("sidevoice.token.{secret}"), "https://evil.example").1, 403);

    // «Volver a conectar»: a new local device; the tunnel opened with the old token is closed.
    assert_eq!(host.reconnect().state, State::Running);
    let mut rest = Vec::new();
    socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let closed = socket.read_to_end(&mut rest).is_ok() || rest.is_empty();
    assert!(closed && rest.is_empty(), "the old tunnel was closed");
}

#[test]
fn a_revoked_token_is_refused_until_reconnect_and_its_tunnels_close() {
    let world = World::new();
    let core = world.core(1);
    let host = world.host();
    let (port, secret) = running(&host);
    let (mut socket, status, _) = ws_handshake(port, &format!("sidevoice.token.{secret}"), "tauri://localhost");
    assert_eq!(status, 101);

    core.revoke_all();
    assert_eq!(host.poll().state, State::Refused);
    assert_eq!(host.pairing(), None);
    let mut rest = Vec::new();
    let _ = socket.read_to_end(&mut rest);
    assert!(rest.is_empty());
    // Never automatic: the core restarting, the poll going on — still refused, nothing paired.
    core.relaunch();
    for _ in 0..3 {
        assert_eq!(host.poll().state, State::Refused);
    }
    assert_eq!(core.local_device(), None);
    let bearer = format!("Bearer {secret}");
    let fields =
        [("Host", format!("127.0.0.1:{port}")), ("Origin", "tauri://localhost".into()), ("Authorization", bearer)];
    let fields: Vec<(&str, &str)> = fields.iter().map(|(n, v)| (*n, v.as_str())).collect();
    assert_eq!(request(port, "GET", "/api/x", &fields, "").0, 503, "nothing carried while refused");

    assert_eq!(host.reconnect().state, State::Running);
    assert!(core.local_device().is_some());
    assert_eq!(request(port, "GET", "/api/x", &fields, "").0, 200, "the same launch's secret works again");
}

#[test]
fn a_stored_pairing_is_reused_after_proving_the_identity() {
    let world = World::new();
    let core = world.core(1);
    let first = world.host();
    running(&first);
    let device = core.local_device();
    first.shutdown();
    drop(first);

    // The app again (a new launch: a new proxy, a new secret), the same core.
    let host = world.host();
    let (_, secret) = running(&host);
    assert_eq!(core.local_device(), device, "no new device");
    assert!(core.requests().iter().any(|r| r.starts_with("GET /api/device/identity")));
    assert_ne!(Some(secret), None);
}

#[test]
fn a_pairing_below_traverse_only_ancestors_loads_saves_and_is_discovered() {
    let world = World::new();
    let core = world.core(1);
    let first = world.host();
    running(&first);
    let stored = store::load(&world.app_dir).unwrap();
    first.shutdown();
    drop(first);

    let mut expected = stored.clone();
    StorageFixtures::with_search_only_ancestor(world._tmp.path(), || {
        assert_eq!(store::load(&world.app_dir), Some(expected.clone()));
        let updated = store::Pairing { host: "saved beneath a searchable ancestor".into(), ..stored.clone() };
        store::save(&world.app_dir, &updated).unwrap();
        assert_eq!(store::load(&world.app_dir), Some(updated.clone()));
        let host = world.host();
        running(&host);
        assert_eq!(core.local_device().as_deref(), Some(updated.device_id.as_str()));
        assert_eq!(store::load(&world.app_dir).unwrap().token, updated.token);
        host.shutdown();
        expected = updated;
    });
}

#[test]
fn a_reset_core_is_paired_again() {
    let world = World::new();
    let core = world.core(1);
    let host = world.host();
    running(&host);
    let old_fp = store::load(&world.app_dir).unwrap().fp;
    core.stop();
    drop(core);

    let reset = world.core(2);
    assert_ne!(reset.fingerprint(), old_fp);
    running(&host);
    assert_eq!(store::load(&world.app_dir).unwrap().fp, reset.fingerprint());
    assert!(reset.local_device().is_some());
}

#[test]
fn a_pairing_that_cannot_be_stored_is_revoked_and_failed() {
    let world = World::new();
    let core = world.core(1);
    std::fs::create_dir_all(world.app_dir.parent().unwrap()).unwrap();
    std::fs::write(&world.app_dir, b"a file where the config directory should be").unwrap();
    let host = world.host();
    let report = serde_json::to_value(host.poll()).unwrap();
    assert_eq!(report["state"], "failed");
    assert_eq!(report["failure"]["key"], "app.storage");
    assert_eq!(core.local_device(), None, "DELETE /api/device/local ran");
    assert!(core.requests().iter().any(|r| r.starts_with("DELETE /api/device/local")));
    assert_eq!(host.pairing(), None);
    // Not retried on every poll.
    let pairs =
        |core: &FakeCore| core.requests().iter().filter(|r| r.starts_with("POST /api/device/local/pair")).count();
    let before = pairs(&core);
    host.poll();
    assert_eq!(pairs(&core), before);
}

#[test]
fn an_incompatible_core_is_not_paired() {
    let world = World::new();
    let core = world.core(1);
    core.set_api(2);
    let host = world.host();
    assert_eq!(host.poll().state, State::Incompatible);
    assert_eq!(core.local_device(), None);
    assert_eq!(host.pairing(), None);
}

#[test]
fn an_unsafe_core_directory_is_never_connected_to() {
    let world = World::new();
    let core = world.core(1);
    let refused = || {
        let report = serde_json::to_value(world.host().poll()).unwrap();
        assert_eq!(report["state"], "failed");
        assert_eq!(report["failure"]["key"], "identity.unsafe-directory");
        assert!(core.requests().is_empty(), "{:?}", core.requests());
        StorageFixtures::private(&world.dirs.core());
    };
    // Readable by the owner's group alone, where the OS has group-only access, and by everybody.
    if temp::let_group_read(&world.dirs.core()) {
        refused();
    }
    StorageFixtures::readable(&world.dirs.core());
    refused();
}

#[test]
fn with_nothing_installed_the_host_is_absent() {
    let world = World::new();
    let host = world.host();
    assert_eq!(
        serde_json::to_value(host.poll()).unwrap(),
        json!({"state": "absent", "product_version": null, "reachable": false})
    );
    assert_eq!(host.act(Action::Start).unwrap_err().key, "cli.unavailable");
}

/// A stand-in CLI recorded in `install.json`: it appends its arguments to `calls.log` and answers per subcommand;
/// `--version --json` with the JSON value `D/version` holds (`"0.8.1"` to begin with), slowly while `D/slow` exists,
/// leaving `D/version-read` once it has read it.
fn install_fake_cli(dirs: &DataDirs, status: &str) -> PathBuf {
    std::fs::create_dir_all(&dirs.data).unwrap();
    StorageFixtures::private(&dirs.data);
    let script = fake_cli(dirs);
    let log = dirs.data.join("calls.log");
    fixture::install(
        Path::new(env!("CARGO_BIN_EXE_fake-connector")),
        &script,
        &json!({"mode":"host","status":serde_json::from_str::<Value>(status).unwrap()}),
    )
    .unwrap();
    std::fs::write(dirs.install_record(), json!({"command": [script]}).to_string()).unwrap();
    StorageFixtures::private(&dirs.install_record());
    set_version(dirs, json!("0.8.1"));
    log
}

/// Where [`install_fake_cli`] puts the stand-in: a native program's name, the only kind Windows trusts (`.exe`).
fn fake_cli(dirs: &DataDirs) -> PathBuf {
    dirs.data.join(format!("fake-cli{}", std::env::consts::EXE_SUFFIX))
}

/// How long [`wait_for`] waits for a stand-in to get somewhere.
const WAIT_LIMIT: Duration = Duration::from_secs(30);

/// Waits until `done` holds; past [`WAIT_LIMIT`] the test fails naming `what`, rather than waiting forever for a step
/// a stand-in that never ran will not take.
fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let started = std::time::Instant::now();
    while !done() {
        assert!(started.elapsed() < WAIT_LIMIT, "{what}: not within {WAIT_LIMIT:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn calls(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log).unwrap_or_default().lines().map(str::to_string).collect()
}

#[test]
fn without_a_connector_the_cli_says_and_actions_run_through_it() {
    let world = World::new();
    let log = install_fake_cli(
        &world.dirs,
        r#"{"ok":true,"state":"stopped-by-person","service":"launchd","installed":true,"core":null,"calls":null,"failure":null,"attempts":null,"limit":null,"since":null,"window_started":null,"next_retry_at":null,"reachable":false,"connector":{"running":false}}"#,
    );
    let host = world.host();
    let report = serde_json::to_value(host.poll()).unwrap();
    assert_eq!(
        report,
        json!({"state": "stopped-by-person", "service": "launchd", "product_version": "0.8.1", "reachable": false}),
        "calls null: unknown"
    );
    host.poll();
    assert_eq!(
        calls(&log),
        ["service status --json", "--version --json"],
        "neither the fallback nor the version is re-run on every poll"
    );

    assert_eq!(host.act(Action::Restart).unwrap().state, State::StoppedByPerson);
    let refused = host.act(Action::Start).unwrap_err();
    assert_eq!(refused.key, "service.not-loaded");
    host.act(Action::Stop).unwrap();
    host.act(Action::ServiceInstall).unwrap();
    host.act(Action::ServiceUninstall).unwrap();
    host.act(Action::LoginOn).unwrap();
    host.act(Action::LoginOff).unwrap();
    assert_eq!(host.pairing_code().unwrap(), json!({"code": "SV1-CODE", "expires_in": 600, "reach": "local-only"}));
    let ran = calls(&log);
    for expected in [
        "service restart --json",
        "service start --json",
        "service stop --json",
        "service install --json",
        "service uninstall --json",
        "service login on --json",
        "service login off --json",
        "pair-device --json",
    ] {
        assert!(ran.iter().any(|c| c == expected), "{expected} not in {ran:?}");
    }
}

#[test]
fn a_connector_without_start_at_sign_in_refuses_it_with_a_key_at_once() {
    let world = World::new();
    let log = install_fake_cli(&world.dirs, r#"{"ok":true,"state":"running","service":"launchd"}"#);
    // As a connector whose command line has no `service login` answers `--json` (clap's error, keyed).
    let script = fake_cli(&world.dirs);
    fixture::configure(
        &script,
        &json!({"mode":"host","status":{"ok":true,"state":"running","service":"launchd"},"unsupported_login":true}),
    )
    .unwrap();
    let host = world.host();
    let started = std::time::Instant::now();
    for action in [Action::LoginOn, Action::LoginOff] {
        let refused = host.act(action).unwrap_err();
        assert_eq!(refused.key, "service.login-unsupported");
    }
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    assert_eq!(host.act(Action::Stop).unwrap().state, State::Starting, "the other actions are untouched");
    let ran = calls(&log);
    assert!(ran.iter().any(|c| c == "service login on --json"), "{ran:?}");
    assert!(ran.iter().any(|c| c == "service login off --json"), "{ran:?}");
}

/// A CLI whose start at sign-in is a fact it keeps (`service login on|off` writes it, `service status` reports it as
/// `start_at_login`); the service itself stays stopped. A `gate` file holds the next `service status` (its answer
/// already read) until a `release` file appears.
fn install_login_cli(dirs: &DataDirs) -> (PathBuf, PathBuf) {
    let log = install_fake_cli(dirs, "{}");
    let dir = dirs.data.clone();
    fixture::configure(&fake_cli(dirs), &json!({"mode":"login"})).unwrap();
    (log, dir)
}

#[test]
fn start_at_sign_in_reads_back_as_installed_and_leaves_the_service_as_it_was() {
    let world = World::new();
    let (log, _) = install_login_cli(&world.dirs);
    let host = world.host();
    let before = host.poll();
    assert_eq!((before.state, before.installed), (State::StoppedByPerson, Some(false)));
    let on = host.act(Action::LoginOn).unwrap();
    assert_eq!((on.state, on.installed), (State::StoppedByPerson, Some(true)), "on: read back at once");
    assert_eq!(host.state(), on, "what subscribers are polled with");
    let off = host.act(Action::LoginOff).unwrap();
    assert_eq!((off.state, off.installed), (State::StoppedByPerson, Some(false)));
    assert_eq!(host.state(), off);
    let ran = calls(&log);
    assert!(ran.iter().all(|c| !c.starts_with("service start") && !c.starts_with("service stop")), "{ran:?}");
}

#[test]
fn a_poll_in_flight_cannot_put_back_the_status_an_action_replaced() {
    let world = World::new();
    let (log, dir) = install_login_cli(&world.dirs);
    let host = world.host();
    // A poll reads `start_at_login: false` and is held there, its answer not stored yet.
    std::fs::write(dir.join("gate"), "").unwrap();
    let polling = std::thread::spawn({
        let host = host.clone();
        move || host.poll()
    });
    wait_for("the poll runs `service status`", || calls(&log).iter().any(|c| c == "service status --json"));
    let acting = std::thread::spawn({
        let host = host.clone();
        move || host.act(Action::LoginOn)
    });
    wait_for("the action runs `service login on`", || calls(&log).iter().any(|c| c == "service login on --json"));
    // Let the action's CLI return and reach the cache before the held poll stores its older answer.
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(dir.join("release"), "").unwrap();
    assert_eq!(polling.join().unwrap().installed, Some(false), "the held poll saw the old fact");
    let after = acting.join().unwrap().unwrap();
    assert_eq!(after.installed, Some(true), "the action's state is read after it, not the held poll's");
    assert_eq!(host.state().installed, Some(true));
    assert_eq!(host.poll().installed, Some(true), "and the cache holds the new answer");
}

#[test]
fn a_connector_that_answers_is_asked_first() {
    let world = World::new();
    let log = install_fake_cli(&world.dirs, r#"{"ok":true,"state":"absent"}"#);
    let listener = fake_core::connector_listener(&world.dirs.data).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut line = String::new();
            let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(&mut stream), &mut line);
            let asked: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(asked["method"], "node.status");
            // SEAMS rev. 2 §4: the derived status, as any connector answers it.
            let result = json!({"ok": true, "state": "backoff", "service": "systemd", "installed": true, "core": null,
                "calls": null, "failure": {"key": "import.missing-module", "detail": "soxr"}, "attempts": 2, "limit": 5,
                "since": null, "window_started": null, "next_retry_at": null, "reachable": false,
                "connector": {"running": true}});
            let _ = writeln!(stream, "{}", json!({"id": asked["id"], "ok": true, "result": result}));
        }
    });
    let host = world.host();
    let report = serde_json::to_value(host.poll()).unwrap();
    assert_eq!(report["state"], "backoff");
    assert_eq!(report["failure"]["detail"], "soxr");
    assert_eq!((report["attempts"].as_u64(), report["limit"].as_u64()), (Some(2), Some(5)));
    assert!(report.get("calls").is_none(), "calls null: unknown");
    assert_eq!(calls(&log), ["--version --json"], "no `service status` while the connector answers");
    assert_eq!(report["product_version"], "0.8.1");
}

#[test]
fn shutdown_closes_every_tunnel() {
    let world = World::new();
    let _core = world.core(1);
    let host = world.host();
    let (port, secret) = running(&host);
    let (mut socket, status, _) = ws_handshake(port, &format!("sidevoice.token.{secret}"), "tauri://localhost");
    assert_eq!(status, 101);
    host.shutdown();
    let mut rest = Vec::new();
    let _ = socket.read_to_end(&mut rest);
    assert!(rest.is_empty());
    std::thread::sleep(Duration::from_millis(100));
    assert!(TcpStream::connect(("127.0.0.1", port)).map(|mut s| s.read(&mut [0u8; 1]).unwrap_or(0)).unwrap_or(0) == 0);
}

/// A core that answers while the service is not what it should be (F6): the state says the service's condition, and
/// the host stays usable (`reachable`, `pairing()`, the proxy).
#[test]
fn a_healthy_core_keeps_the_services_condition_and_stays_usable() {
    for (reported, expected) in [
        (
            r#"{"ok":true,"state":"not-installed","service":"none","installed":false,"failure":null,"calls":0,"core":null}"#,
            "not-installed",
        ),
        (
            r#"{"ok":true,"state":"service-failed","service":"launchd","installed":true,"failure":{"key":"start-limit"},"calls":0,"core":null}"#,
            "service-failed",
        ),
    ] {
        let world = World::new();
        let _core = world.core(1);
        install_fake_cli(&world.dirs, reported);
        let host = world.host();
        let report = serde_json::to_value(host.poll()).unwrap();
        assert_eq!(report["state"], expected, "{report}");
        assert_eq!(report["reachable"], true);
        if expected == "service-failed" {
            assert_eq!(report["failure"]["key"], "start-limit");
        }
        let pairing = host.pairing().expect("usable whatever the state says");
        let port = host.proxy().port();
        let bearer = format!("Bearer {}", pairing["token"].as_str().unwrap());
        let host_field = format!("127.0.0.1:{port}");
        let fields =
            [("Host", host_field.as_str()), ("Origin", "tauri://localhost"), ("Authorization", bearer.as_str())];
        assert_eq!(request(port, "GET", "/api/x", &fields, "").0, 200);
    }
}

/// Any connector's `node.status` is the derived service status (SEAMS rev. 2 §4), with or without a service: the CLI is
/// not asked, and the service condition still wins over a core that answers.
#[test]
fn a_connector_status_is_the_service_status() {
    let world = World::new();
    let _core = world.core(1);
    let log = install_fake_cli(&world.dirs, r#"{"ok":true,"state":"absent"}"#);
    let listener = fake_core::connector_listener(&world.dirs.data).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut line = String::new();
            let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(&mut stream), &mut line);
            let result = json!({"ok": true, "state": "not-installed", "service": "none", "installed": false,
                "core": {"pid": 1, "version": "0.0.0-fake", "api": 1, "launch_id": null}, "calls": 0, "failure": null,
                "attempts": null, "limit": null, "since": null, "window_started": null, "next_retry_at": null,
                "reachable": true, "connector": {"running": true}});
            let _ = writeln!(stream, "{}", json!({"id": 1, "ok": true, "result": result}));
        }
    });
    let host = world.host();
    let report = serde_json::to_value(host.poll()).unwrap();
    assert_eq!((report["state"].as_str(), report["reachable"].as_bool()), (Some("not-installed"), Some(true)));
    assert!(host.pairing().is_some());
    assert_eq!(calls(&log), ["--version --json"], "no `service status` while a connector answers");
    assert_eq!(report["product_version"], "0.8.1", "the installed version, not the core's 0.0.0-fake");
}

/// What the stand-in CLI's `--version --json` answers as `version`: any JSON value.
fn set_version(dirs: &DataDirs, version: Value) {
    std::fs::write(dirs.data.join("version"), version.to_string()).unwrap();
}

/// The installed Sidevoice's version (`product_version`): the installation's program asked once and kept, asked again
/// when `install.json`'s command resolves to another program and after an action; `null` with nothing installed or an
/// answer that is not a version.
#[test]
fn the_installed_version_is_asked_once_per_program_and_after_actions() {
    let world = World::new();
    let core = world.core(1);
    let log = install_fake_cli(&world.dirs, r#"{"ok":true,"state":"running","service":"launchd"}"#);
    let host = world.host();
    let asked = || calls(&log).iter().filter(|c| *c == "--version --json").count();
    assert_eq!(host.poll().product_version.as_deref(), Some("0.8.1"));
    host.poll();
    core.relaunch();
    host.poll();
    assert_eq!(asked(), 1, "kept while the installed program is the same, whatever the core does");
    assert_eq!(host.state().product_version.as_deref(), Some("0.8.1"));

    // An update selects another release: the command resolves to another program, which is asked.
    let release = world.dirs.data.join(format!("fake-cli-0.9.0{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(fake_cli(&world.dirs), &release).unwrap();
    std::fs::copy(fixture::config_path(&fake_cli(&world.dirs)), fixture::config_path(&release)).unwrap();
    std::fs::write(world.dirs.install_record(), json!({"command": [release]}).to_string()).unwrap();
    set_version(&world.dirs, json!("0.9.0"));
    assert_eq!(host.poll().product_version.as_deref(), Some("0.9.0"));
    assert_eq!(asked(), 2);

    // An action asks again.
    set_version(&world.dirs, json!("0.9.1-rc.1+abc"));
    assert_eq!(host.act(Action::Restart).unwrap().product_version.as_deref(), Some("0.9.1-rc.1+abc"));
    assert_eq!(asked(), 3);

    // What is not a version is unknown, never passed on; the core's own version stays where it was.
    let long = format!("1.2.3-{}", "a".repeat(60));
    for bad in [
        json!("unknown"),
        json!(""),
        json!(" "),
        json!("0.8.1_dirty"),
        json!("0.8.1 "),
        json!("ü.1.2"),
        json!(long),
        json!(7),
    ] {
        set_version(&world.dirs, bad.clone());
        let report = serde_json::to_value(host.act(Action::Restart).unwrap()).unwrap();
        assert_eq!(report["product_version"], Value::Null, "{bad}");
        assert_eq!(report["core"]["version"], "0.0.0-fake");
    }
    // Unknown is not asked again on every poll.
    let before = asked();
    host.poll();
    assert_eq!(asked(), before, "unknown is asked again only after a while");

    // Uninstalled while nothing else changes (no core, the same launch seen): null at once, nothing run.
    set_version(&world.dirs, json!("0.9.1"));
    host.act(Action::Restart).unwrap();
    core.stop();
    assert_eq!(host.poll().product_version.as_deref(), Some("0.9.1"));
    std::fs::remove_file(world.dirs.install_record()).unwrap();
    let before = asked();
    assert_eq!(serde_json::to_value(host.poll()).unwrap()["product_version"], Value::Null);
    assert_eq!(asked(), before);
}

/// A poll under way when an action ends cannot leave its pre-action answer behind: the action's own pass asks again.
#[test]
fn an_action_during_a_poll_still_refreshes_the_version() {
    let world = World::new();
    let _core = world.core(1);
    install_fake_cli(&world.dirs, r#"{"ok":true,"state":"running","service":"launchd"}"#);
    let (slow, read) = (world.dirs.data.join("slow"), world.dirs.data.join("version-read"));
    std::fs::write(&slow, "").unwrap();
    let host = world.host();
    let polling = {
        let host = host.clone();
        std::thread::spawn(move || host.poll().product_version)
    };
    // The poll has read the old version and is still answering.
    wait_for("the poll reads the version", || read.exists());
    set_version(&world.dirs, json!("0.9.0"));
    std::fs::remove_file(&slow).unwrap();
    assert_eq!(host.act(Action::Restart).unwrap().product_version.as_deref(), Some("0.9.0"));
    assert_eq!(polling.join().unwrap().as_deref(), Some("0.8.1"));
    assert_eq!(host.state().product_version.as_deref(), Some("0.9.0"));
}

/// «Ver registro»: the core's log, else the connector's; the old supervisor log is not one of them.
#[test]
fn the_log_shown_is_the_cores_then_the_connectors() {
    let world = World::new();
    std::fs::create_dir_all(&world.dirs.data).unwrap();
    let host = world.host();
    std::fs::write(world.dirs.data.join("node-service.log"), b"old").unwrap();
    assert_eq!(host.log_path(), None);
    std::fs::write(world.dirs.data.join("connector.log"), b"c").unwrap();
    assert_eq!(host.log_path(), Some(world.dirs.data.join("connector.log")));
    std::fs::write(world.dirs.data.join("core.log"), b"k").unwrap();
    assert_eq!(host.log_path(), Some(world.dirs.data.join("core.log")));
}

/// Clients that never finish a head — no secret, no Origin needed to open a connection — cannot hold every slot, and
/// none of them keeps its connection past the head deadline (review R1-c #3).
#[test]
fn slow_clients_cannot_starve_the_page() {
    let world = World::new();
    let _core = world.core(1);
    let host = world.host();
    let (port, secret) = running(&host);
    let mut slow = Vec::new();
    for _ in 0..256 {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(b"GET /api/x HTTP/1.1\r\nHost: 127").unwrap();
        slow.push(stream);
    }
    // Each trickles one more byte, as one that dodges a per-read timeout would; then they go quiet. Once idle for a
    // second they are what makes room.
    for stream in &mut slow {
        let _ = stream.write_all(b".");
    }
    std::thread::sleep(Duration::from_millis(1300));
    let started = std::time::Instant::now();
    let host_field = format!("127.0.0.1:{port}");
    let bearer = format!("Bearer {secret}");
    let fields = [("Host", host_field.as_str()), ("Origin", "tauri://localhost"), ("Authorization", bearer.as_str())];
    assert_eq!(request(port, "GET", "/api/x", &fields, "").0, 200, "the page gets through");
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    drop(slow);

    // One that keeps trickling is closed at the head deadline (5 s), not kept alive byte by byte.
    let mut trickler = TcpStream::connect(("127.0.0.1", port)).unwrap();
    trickler.set_read_timeout(Some(Duration::from_millis(400))).unwrap();
    let started = std::time::Instant::now();
    let closed = loop {
        if trickler.write_all(b"x").is_err() {
            break started.elapsed();
        }
        match trickler.read(&mut [0u8; 64]) {
            Ok(0) => break started.elapsed(),
            Ok(_) => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => break started.elapsed(),
        }
        assert!(started.elapsed() < Duration::from_secs(9), "never closed");
    };
    assert!(closed >= Duration::from_secs(4) && closed < Duration::from_secs(8), "{closed:?}");
}

/// A socket's bytes go to the core only after its 101: data sent behind the upgrade head is refused here, and the
/// core never sees the request (review R1-c #4).
#[test]
fn data_before_the_upgrade_is_refused() {
    let world = World::new();
    let core = world.core(1);
    let host = world.host();
    let (port, secret) = running(&host);
    let seen = core.requests().len();
    let raw = format!(
        "GET /api/browser/call HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: tauri://localhost\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Protocol: sidevoice.token.{secret}\r\n\r\nGET /api/x HTTP/1.1\r\n\r\n"
    );
    assert_eq!(send(port, &raw).0, 400);
    let chunked = raw.replace("\r\n\r\nGET /api/x HTTP/1.1\r\n\r\n", "\r\nTransfer-Encoding: chunked\r\n\r\n");
    assert_eq!(send(port, &chunked).0, 411);
    assert_eq!(core.requests().len(), seen, "{:?}", core.requests());
}

/// A valid head that arrives in pieces is never cut short to admit newcomers: connection churn by clients with no
/// secret is refused instead (review R1-c #3, re-check).
#[test]
fn a_head_still_arriving_survives_connection_churn() {
    let world = World::new();
    let _core = world.core(1);
    let host = world.host();
    let (port, secret) = running(&host);
    let mut page = TcpStream::connect(("127.0.0.1", port)).unwrap();
    page.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let head = format!(
        "GET /api/x HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: tauri://localhost\r\nAuthorization: Bearer {secret}\r\n"
    );
    page.write_all(head.as_bytes()).unwrap();
    // Churn meanwhile: far more unauthenticated connections than there are head slots, each sending a byte. The page
    // finishes its head 300 ms in (well within the idle second), whatever the churn has reached by then.
    let churn = std::thread::spawn(move || {
        let mut churn = Vec::new();
        for _ in 0..200 {
            if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
                let _ = stream.write_all(b"G");
                churn.push(stream);
            }
        }
        churn
    });
    std::thread::sleep(Duration::from_millis(300));
    page.write_all(b"\r\n").unwrap();
    let mut answer = String::new();
    let _ = page.read_to_string(&mut answer);
    drop(churn.join());
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer:?}");
}

// ---- the install: the package's own installer, then the app paired with the core it started ----

const PLATFORM: &str = "@sidevoice/sidevoice-linux-x64";

fn install_options(world: &World) -> install::Options {
    let temp = world._tmp.path().join("t");
    std::fs::create_dir_all(&temp).unwrap();
    install::Options {
        package: Some(PLATFORM.into()),
        temp,
        temp_often_redirected: false,
        installer_time: Duration::from_secs(20),
        settle_time: Duration::from_secs(5),
        ..install::Options::standard()
    }
}

/// A release whose installer answers as the real one does once the service runs, after `seconds`.
fn release(seconds: &str) -> FakeRegistry {
    fixture::release(
        Path::new(env!("CARGO_BIN_EXE_fake-connector")),
        "0.8.1",
        PLATFORM,
        &json!({"delay_ms":(seconds.parse::<f64>().unwrap()*1000.0) as u64,"response":{"ok":true,"action":"installed","version":"0.8.1","state":"running"}}),
    )
}

#[test]
fn an_install_runs_alone_says_installing_meanwhile_and_ends_with_the_core_reachable() {
    let world = World::new();
    // The core the installer starts (here, already up: the stand-in installer only answers).
    let core = world.core(1);
    let host = world.host();
    let registry = release("1");
    let options = install_options(&world);
    let installing = host.begin_install("job-1").unwrap();
    assert_eq!(installing.id(), "job-1");
    assert_eq!(host.state().state, State::Installing, "at once, before any pass");
    let busy = host.begin_install("job-2").map(|_| ()).unwrap_err();
    assert_eq!(busy.key, install::BUSY);
    assert_eq!(host.begin_install("").map(|_| ()).unwrap_err().key, "bad_request");
    let report = std::thread::scope(|scope| {
        let running = scope.spawn(|| installing.run(&registry, &options));
        let mut seen = Vec::new();
        while !running.is_finished() {
            // Only polls the install was under way for all along (its slot frees once, before its last pass).
            let step = host.install_progress("job-1").map(|p| p.step);
            let state = host.poll().state;
            if step.is_some() && host.install_progress("job-1").is_some() {
                seen.push((state, step));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(seen.iter().any(|(state, _)| *state == State::Installing), "{seen:?}");
        assert!(seen.iter().all(|(state, _)| *state == State::Installing), "nothing else while it runs: {seen:?}");
        assert!(seen.iter().any(|(_, step)| *step == Some(install::Step::Staging)), "{seen:?}");
        assert_eq!(host.install_progress("job-2"), None);
        running.join().unwrap()
    })
    .unwrap();
    assert_eq!((report.state, report.reachable), (State::Running, true), "{report:?}");
    assert_eq!(host.pairing().unwrap()["fp"], core.fingerprint());
    assert_eq!(host.install_progress("job-1"), None, "over");
    assert!(!host.cancel_install("job-1"));
    // Free again; one begun and dropped unrun frees it too.
    drop(host.begin_install("job-3").unwrap());
    drop(host.begin_install("job-4").unwrap());
    assert_eq!(std::fs::read_dir(&options.temp).unwrap().count(), 0, "nothing left behind");
}

#[test]
fn an_install_over_a_refused_pairing_pairs_again() {
    let world = World::new();
    let core = world.core(1);
    let host = world.host();
    running(&host);
    let before = store::load(&world.app_dir).unwrap().device_id;
    core.revoke_all();
    assert_eq!(host.poll().state, State::Refused);
    // The installer restarts the core: a new launch, the same identity (an update keeps it).
    core.relaunch();
    let report = host.begin_install("again").unwrap().run(&release("0"), &install_options(&world)).unwrap();
    assert_eq!((report.state, report.reachable), (State::Running, true), "{report:?}");
    assert_ne!(store::load(&world.app_dir).unwrap().device_id, before, "a new local device");
}

#[test]
fn a_failed_install_rejects_and_the_state_is_what_is_there() {
    let world = World::new();
    let host = world.host();
    let registry = release("0");
    registry.serve(&registry.tarball_url(), 200, b"not the tarball".to_vec());
    let failure = host.begin_install("bad").unwrap().run(&registry, &install_options(&world)).unwrap_err();
    assert_eq!((failure.key.as_str(), failure.step), ("install.checksum", Some(install::Step::Verification)));
    assert_eq!(host.state().state, State::Absent);
    assert!(host.begin_install("next").is_ok(), "free again");
}

#[test]
fn a_cancel_is_honoured_by_id_until_the_installer_runs() {
    let world = World::new();
    let host = world.host();
    let registry = release("0");
    registry.slow(&registry.tarball_url(), Duration::from_millis(30));
    let installing = host.begin_install("slow").unwrap();
    let options = install_options(&world);
    let failure = std::thread::scope(|scope| {
        let running = scope.spawn(|| installing.run(&registry, &options));
        wait_for("the download makes progress", || host.install_progress("slow").and_then(|p| p.done).is_some());
        assert!(!host.cancel_install("another"), "only its own id");
        assert!(host.cancel_install("slow"));
        running.join().unwrap()
    })
    .unwrap_err();
    assert_eq!((failure.key.as_str(), failure.step), (install::CANCELLED, Some(install::Step::Download)));
    assert_ne!(host.state().state, State::Installing);
}

// ---- the service's version and its updates, within the minor the app pins ----

/// The minor the stand-in bundled page pins.
const PINNED: &str = "0.20";
const STATUS_RUNNING: &str = r#"{"ok":true,"state":"running","service":"launchd"}"#;

impl World {
    /// A host for an app that pins [`PINNED`] (`None`: a web release that pins nothing).
    fn host_pinning(&self, minor: Option<&str>) -> Arc<LocalHost> {
        let pinned_minor = minor.map(|minor| Minor::parse(minor).unwrap());
        let config =
            Config { dirs: self.dirs.clone(), app_dir: self.app_dir.clone(), name: "test-mac".into(), pinned_minor };
        let log = self.log.clone();
        LocalHost::start(config, move |line| log.lock().unwrap().push(line.to_string())).unwrap()
    }

    /// Where the stand-in installer leaves its arguments when it runs.
    fn installer_ran(&self) -> PathBuf {
        self._tmp.path().join("installer-ran")
    }

    /// The registry after 0.20.2: it lists 0.19.9 to 0.21.0, and the installer of its release, once it runs, makes the
    /// installed program say 0.20.2 (as the real one does by switching `R/current`), after `installer_seconds`.
    fn registry_at_0_20_2(&self, installer_seconds: f64) -> FakeRegistry {
        self.registry_whose_installer(installer_seconds, true)
    }

    /// The same, whose installer answers ok and, when `changes_version` is false, leaves the installed program as it was.
    fn registry_whose_installer(&self, installer_seconds: f64, changes_version: bool) -> FakeRegistry {
        let mut installer = json!({
            "ran": self.installer_ran(), "delay_ms": (installer_seconds * 1000.0) as u64,
            "response": {"ok": true, "action": "updated", "version": "0.20.2", "state": "running"},
        });
        if changes_version {
            installer["set_version"] = json!("0.20.2");
        }
        let registry =
            fixture::release(Path::new(env!("CARGO_BIN_EXE_fake-connector")), "0.20.2", PLATFORM, &installer);
        registry.list(&["0.19.9", "0.20.0", "0.20.1", "0.20.2", "0.21.0"]);
        registry
    }
}

/// An installed service of `version`, running, with its core up.
fn installed(world: &World, version: &str) -> Arc<FakeCore> {
    let core = world.core(1);
    install_fake_cli(&world.dirs, STATUS_RUNNING);
    set_version(&world.dirs, json!(version));
    core
}

fn updated_to(host: &LocalHost) -> Option<String> {
    host.version()["installed"]["version"].as_str().map(str::to_string)
}

fn tarball_fetches(registry: &FakeRegistry) -> usize {
    registry.asked().iter().filter(|url| **url == registry.tarball_url()).count()
}

/// Stops the host when dropped, so a failed assertion in a test does not leave its background update running (the
/// test's thread scope would wait for it for ever).
struct StopsHost<'a>(&'a LocalHost);

impl Drop for StopsHost<'_> {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

/// Runs the background update (`keep_current`) beside `body`, polling as the app does, and stops it after.
fn keeping_current(host: &Arc<LocalHost>, registry: &FakeRegistry, options: &install::Options, body: impl FnOnce()) {
    host.watch();
    std::thread::scope(|scope| {
        let _stops = StopsHost(host);
        scope.spawn(|| host.keep_current(registry, options, || Some(())));
        body();
    });
}

#[test]
fn version_says_what_is_installed_against_the_pin_and_nothing_without_one() {
    let world = World::new();
    installed(&world, "0.20.0");
    let host = world.host_pinning(Some(PINNED));
    host.poll();
    assert_eq!(
        host.version(),
        json!({
            "bundled": {"minor": "0.20"}, "installed": {"version": "0.20.0"}, "core_api": 1,
            "update": "current", "latest": null, "progress": null,
        }),
        "the registry is not known to have anything newer yet"
    );
    let unpinned = world.host_pinning(None);
    unpinned.poll();
    let version = unpinned.version();
    assert_eq!(
        (&version["update"], &version["bundled"], &version["installed"]["version"]),
        (&json!("unknown"), &json!({}), &json!("0.20.0"))
    );
    let nothing = World::new();
    let empty = nothing.host_pinning(Some(PINNED));
    empty.poll();
    assert_eq!(empty.version()["update"], "unknown");
    assert_eq!(empty.version()["installed"], Value::Null);
}

#[test]
fn an_update_installs_the_newest_patch_of_the_pinned_minor_and_never_the_latest() {
    let world = World::new();
    installed(&world, "0.20.0");
    let host = world.host_pinning(Some(PINNED));
    host.poll();
    let registry = world.registry_at_0_20_2(0.0);
    let report = host.update(&registry, &install_options(&world)).unwrap();
    assert_eq!(report.product_version.as_deref(), Some("0.20.2"), "{report:?}");
    let version = host.version();
    assert_eq!(
        (&version["update"], &version["latest"], &version["installed"]["version"]),
        (&json!("current"), &json!("0.20.2"), &json!("0.20.2"))
    );
    let asked = registry.asked();
    assert!(asked.contains(&"https://registry.npmjs.org/sidevoice".to_string()), "{asked:?}");
    assert!(asked.contains(&"https://registry.npmjs.org/sidevoice/0.20.2".to_string()), "{asked:?}");
    assert!(!asked.iter().any(|url| url.ends_with("/sidevoice/latest")), "the latest may be another minor: {asked:?}");
    assert!(world.installer_ran().exists());
    // Nothing behind it any more: asking again installs nothing.
    std::fs::remove_file(world.installer_ran()).unwrap();
    host.update(&registry, &install_options(&world)).unwrap();
    assert!(!world.installer_ran().exists(), "up to date: the installer is not run again");
}

#[test]
fn a_service_of_a_newer_minor_is_never_touched() {
    let world = World::new();
    installed(&world, "0.21.0");
    let host = world.host_pinning(Some(PINNED));
    host.poll();
    let registry = world.registry_at_0_20_2(0.0);
    let refused = host.update(&registry, &install_options(&world)).unwrap_err();
    assert_eq!(refused.key, "update.newer-installed");
    assert_eq!(host.version()["update"], "newer-installed");
    keeping_current(&host, &registry, &install_options(&world), || {
        wait_for("the registry is asked", || host.version()["latest"] == "0.20.2");
        std::thread::sleep(Duration::from_millis(2500));
        assert_eq!(host.version()["update"], "newer-installed");
    });
    assert_eq!(tarball_fetches(&registry), 0);
    assert!(!world.installer_ran().exists());
}

#[test]
fn an_older_service_is_updated_in_the_background_without_saying_installing() {
    let world = World::new();
    installed(&world, "0.20.0");
    let host = world.host_pinning(Some(PINNED));
    let registry = world.registry_at_0_20_2(1.5);
    keeping_current(&host, &registry, &install_options(&world), || {
        let mut seen = Vec::new();
        wait_for("the update has run", || {
            let version = host.version();
            seen.push((host.state().state, version["update"].clone(), version["progress"].clone()));
            updated_to(&host).as_deref() == Some("0.20.2")
        });
        assert!(seen.iter().any(|(_, update, _)| update == "updating"), "{seen:?}");
        assert!(seen.iter().all(|(state, _, _)| *state != State::Installing), "the service keeps its state: {seen:?}");
        let progress: Vec<f64> = seen.iter().filter_map(|(_, _, progress)| progress.as_f64()).collect();
        assert!(!progress.is_empty() && progress.iter().all(|p| (0.0..=1.0).contains(p)), "{seen:?}");
        assert!(progress.windows(2).all(|pair| pair[0] <= pair[1]), "forward only: {progress:?}");
        assert!(seen.iter().filter(|(_, update, _)| update != "updating").all(|(_, _, progress)| progress.is_null()));
    });
    assert_eq!(host.version()["update"], "current");
    assert_eq!(tarball_fetches(&registry), 1);
}

#[test]
fn an_update_waits_for_the_call_to_end_and_runs_after() {
    let world = World::new();
    installed(&world, "0.20.0");
    let host = world.host_pinning(Some(PINNED));
    let (port, secret) = running(&host);
    let (call, status, _) = ws_handshake(port, &format!("sidevoice, sidevoice.token.{secret}"), "tauri://localhost");
    assert_eq!(status, 101);
    assert_eq!(host.poll().calls, Some(1), "the host knows a call is on");
    let registry = world.registry_at_0_20_2(0.0);
    keeping_current(&host, &registry, &install_options(&world), || {
        wait_for("it says it waits", || host.version()["update"] == "waiting-call");
        // An explicit update meanwhile changes nothing either, and says so.
        let report = host.update(&registry, &install_options(&world)).unwrap();
        assert_eq!(report.product_version.as_deref(), Some("0.20.0"));
        std::thread::sleep(Duration::from_millis(2500));
        assert_eq!(tarball_fetches(&registry), 0, "nothing was downloaded while the call was on");
        assert_eq!(host.version()["update"], "waiting-call");
        drop(call);
        wait_for("the update runs once the call has ended", || updated_to(&host).as_deref() == Some("0.20.2"));
    });
    assert_eq!(tarball_fetches(&registry), 1);
}

#[test]
fn a_service_the_person_stopped_is_not_started_by_an_update() {
    let world = World::new();
    install_fake_cli(&world.dirs, r#"{"ok":true,"state":"stopped-by-person","service":"launchd"}"#);
    set_version(&world.dirs, json!("0.20.0"));
    let host = world.host_pinning(Some(PINNED));
    let registry = world.registry_at_0_20_2(0.0);
    keeping_current(&host, &registry, &install_options(&world), || {
        wait_for("the registry is asked", || host.version()["latest"] == "0.20.2");
        std::thread::sleep(Duration::from_millis(2500));
    });
    assert_eq!(host.state().state, State::StoppedByPerson);
    assert_eq!(host.version()["update"], "available", "behind, and left alone");
    assert_eq!(tarball_fetches(&registry), 0);
}

#[test]
fn a_failed_update_is_not_retried_every_pass_and_by_hand_it_is() {
    let world = World::new();
    installed(&world, "0.20.0");
    let host = world.host_pinning(Some(PINNED));
    let registry = world.registry_at_0_20_2(0.0);
    registry.serve(&registry.tarball_url(), 200, b"not the tarball".to_vec());
    keeping_current(&host, &registry, &install_options(&world), || {
        wait_for("it tried", || tarball_fetches(&registry) == 1);
        std::thread::sleep(Duration::from_millis(5000));
    });
    assert_eq!(tarball_fetches(&registry), 1, "the same release is left alone for a while");
    assert!(world.log.lock().unwrap().iter().any(|line| line.contains("service update failed: install.checksum")));
    assert_eq!(updated_to(&host).as_deref(), Some("0.20.0"));
    assert_eq!(host.update(&registry, &install_options(&world)).unwrap_err().key, "install.checksum");
    assert_eq!(tarball_fetches(&registry), 2, "the person's own update always tries");
}

#[test]
fn the_install_the_person_asks_for_installs_the_pinned_minor_too() {
    let world = World::new();
    let core = world.core(1);
    let host = world.host_pinning(Some(PINNED));
    let registry = world.registry_at_0_20_2(0.0);
    let report = host.begin_install("first").unwrap().run(&registry, &install_options(&world)).unwrap();
    assert_eq!(report.state, State::Running, "{report:?}");
    assert_eq!(host.pairing().unwrap()["fp"], core.fingerprint());
    let asked = registry.asked();
    assert!(!asked.iter().any(|url| url.ends_with("/sidevoice/latest")), "{asked:?}");
    assert!(asked.contains(&"https://registry.npmjs.org/sidevoice/0.20.2".to_string()), "{asked:?}");
}

#[test]
fn a_call_that_begins_during_the_download_stops_the_update_before_its_installer() {
    let world = World::new();
    installed(&world, "0.20.0");
    let host = world.host_pinning(Some(PINNED));
    let (port, secret) = running(&host);
    let registry = world.registry_at_0_20_2(0.0);
    let (call, status, _) = ws_handshake(port, &format!("sidevoice, sidevoice.token.{secret}"), "tauri://localhost");
    assert_eq!(status, 101);
    assert_eq!(host.state().calls, Some(0), "the host's last pass is from before the call: it does not know yet");
    let report = host.update(&registry, &install_options(&world)).unwrap();
    assert_eq!(report.product_version.as_deref(), Some("0.20.0"), "nothing changed");
    assert_eq!(tarball_fetches(&registry), 1, "it was downloaded before the call was seen");
    assert!(!world.installer_ran().exists(), "but the installer, which restarts the service, never ran");
    assert_eq!(host.version()["update"], "waiting-call");
    assert_eq!(host.state().state, State::Running);
    drop(call);
    // The core notices the closed connection a moment later: the update is asked again only once the host has seen the
    // call end (waiting on that, not on time).
    wait_for("the host sees the call end", || host.poll().calls == Some(0));
    let report = host.update(&registry, &install_options(&world)).unwrap();
    assert_eq!(report.product_version.as_deref(), Some("0.20.2"), "{report:?}");
}

#[test]
fn an_unknown_count_of_calls_is_never_taken_for_none() {
    let world = World::new();
    let core = installed(&world, "0.20.0");
    core.hide_calls(true);
    let host = world.host_pinning(Some(PINNED));
    host.poll();
    let registry = world.registry_at_0_20_2(0.0);
    // Behind, the core up and not saying how many calls are on: it waits, as with one on.
    let report = host.update(&registry, &install_options(&world)).unwrap();
    assert_eq!(report.product_version.as_deref(), Some("0.20.0"));
    assert_eq!(host.version()["update"], "waiting-call");
    assert_eq!(host.update_hold().expect("held").key, install::WAITING_CALL);
    keeping_current(&host, &registry, &install_options(&world), || {
        std::thread::sleep(Duration::from_millis(4500));
    });
    assert_eq!(tarball_fetches(&registry), 0, "nothing was downloaded, let alone installed");
    assert!(!world.installer_ran().exists());
    // Once the core says, it is an ordinary update.
    core.hide_calls(false);
    host.poll();
    assert_eq!(host.version()["update"], "available");
    assert!(host.update_hold().is_none());
    let report = host.update(&registry, &install_options(&world)).unwrap();
    assert_eq!(report.product_version.as_deref(), Some("0.20.2"), "{report:?}");
}

#[test]
fn with_no_core_answering_there_is_no_call_to_wait_for() {
    let world = World::new();
    install_fake_cli(&world.dirs, STATUS_RUNNING);
    set_version(&world.dirs, json!("0.20.0"));
    let host = world.host_pinning(Some(PINNED));
    host.poll();
    let registry = world.registry_at_0_20_2(0.0);
    let options = install::Options { settle_time: Duration::from_millis(200), ..install_options(&world) };
    let report = host.update(&registry, &options).unwrap();
    assert_eq!(report.product_version.as_deref(), Some("0.20.2"), "{report:?}");
}

#[test]
fn the_last_check_says_why_an_update_does_not_go_on() {
    let world = World::new();
    let core = installed(&world, "0.20.0");
    let host = world.host_pinning(Some(PINNED));
    let (port, secret) = running(&host);
    assert!(host.update_hold().is_none(), "nothing in the way");
    // A call is on.
    let (call, status, _) = ws_handshake(port, &format!("sidevoice, sidevoice.token.{secret}"), "tauri://localhost");
    assert_eq!(status, 101);
    assert_eq!(host.update_hold().expect("held").key, install::WAITING_CALL);
    drop(call);
    wait_for("the call has ended", || host.update_hold().is_none());
    // The count is not known.
    core.hide_calls(true);
    assert_eq!(host.update_hold().expect("held").key, install::WAITING_CALL);
    core.hide_calls(false);
    assert!(host.update_hold().is_none());
    // The person stopped the service since (the connector says so; asked again, not read from the last poll).
    let program = fake_cli(&world.dirs);
    let says = |state: &str| {
        fixture::configure(
            &program,
            &json!({"mode": "host", "status": {"ok": true, "state": state, "service": "launchd"}}),
        )
        .unwrap()
    };
    says("stopped-by-person");
    assert_eq!(host.update_hold().expect("held").key, install::STOPPED);
    says("running");
    assert!(host.update_hold().is_none());
    // Uninstalled since.
    std::fs::remove_file(world.dirs.install_record()).unwrap();
    assert_eq!(host.update_hold().expect("held").key, install::GONE);
    for key in [install::WAITING_CALL, install::STOPPED, install::GONE] {
        assert!(install::is_held(key));
    }
}

#[test]
fn a_service_action_cancels_an_update_before_its_installer_and_is_refused_after() {
    let world = World::new();
    installed(&world, "0.20.0");
    let host = world.host_pinning(Some(PINNED));
    host.poll();
    let options = install_options(&world);

    // While it downloads: the action goes first, the update is cancelled and nothing was installed.
    let slow = world.registry_at_0_20_2(0.0);
    slow.slow(&slow.tarball_url(), Duration::from_millis(30));
    let failure = std::thread::scope(|scope| {
        let updating = scope.spawn(|| host.update(&slow, &options));
        wait_for("the download is under way", || {
            host.install_progress("service-update").and_then(|progress| progress.done).is_some()
        });
        host.act(Action::Stop).expect("the person's stop is not refused before the installer");
        updating.join().unwrap().unwrap_err()
    });
    assert_eq!(failure.key, install::CANCELLED);
    assert!(!world.installer_ran().exists(), "the installer never ran: the stop is not undone");
    assert!(calls(&world.dirs.data.join("calls.log")).iter().any(|call| call == "service stop --json"));
    assert_ne!(host.version()["update"], "updating");

    // Once its installer runs: refused, like a second install; the update goes on.
    let registry = world.registry_at_0_20_2(1.5);
    let report = std::thread::scope(|scope| {
        let updating = scope.spawn(|| host.update(&registry, &options));
        wait_for("the installer is running", || {
            host.install_progress("service-update").is_some_and(|progress| !progress.cancellable)
        });
        assert_eq!(host.act(Action::Stop).unwrap_err().key, install::BUSY);
        updating.join().unwrap().unwrap()
    });
    assert_eq!(report.product_version.as_deref(), Some("0.20.2"), "{report:?}");
}

#[test]
fn updates_asked_together_or_beside_the_background_one_never_refuse_each_other() {
    let world = World::new();
    installed(&world, "0.20.0");
    let host = world.host_pinning(Some(PINNED));
    host.poll();
    let registry = world.registry_at_0_20_2(1.0);
    let options = install_options(&world);
    // Two of the person's own at once: one installs, the other waits for it and sees nothing left to do.
    let answers = std::thread::scope(|scope| {
        let first = scope.spawn(|| host.update(&registry, &options));
        let second = scope.spawn(|| host.update(&registry, &options));
        [first.join().unwrap(), second.join().unwrap()]
    });
    for answer in answers {
        assert_eq!(answer.expect("not refused as busy").product_version.as_deref(), Some("0.20.2"));
    }
    assert_eq!(tarball_fetches(&registry), 1, "installed once");

    // And one asked while the background update runs.
    let other = World::new();
    installed(&other, "0.20.0");
    let host = other.host_pinning(Some(PINNED));
    let registry = other.registry_at_0_20_2(1.0);
    keeping_current(&host, &registry, &install_options(&other), || {
        wait_for("the background update has begun", || host.install_progress("service-update").is_some());
        let report = host.update(&registry, &install_options(&other)).expect("not refused as busy");
        assert_eq!(report.product_version.as_deref(), Some("0.20.2"), "{report:?}");
    });
    assert_eq!(tarball_fetches(&registry), 1, "installed once, by whichever came first");
}

#[test]
fn an_installer_that_says_ok_and_changes_nothing_is_not_asked_again_at_every_pass() {
    let world = World::new();
    installed(&world, "0.20.0");
    let host = world.host_pinning(Some(PINNED));
    let registry = world.registry_whose_installer(0.0, false);
    keeping_current(&host, &registry, &install_options(&world), || {
        wait_for("it tried", || tarball_fetches(&registry) == 1);
        std::thread::sleep(Duration::from_millis(5000));
    });
    assert_eq!(tarball_fetches(&registry), 1, "left alone after it, not run again every pass");
    assert!(world.log.lock().unwrap().iter().any(|line| line.contains("service update failed: update.unchanged")));
    assert_eq!(host.version()["update"], "available", "and still said to be behind");
    // The person's own update says so and tries again.
    assert_eq!(host.update(&registry, &install_options(&world)).unwrap_err().key, "update.unchanged");
    assert_eq!(tarball_fetches(&registry), 2);
}

#[test]
fn the_install_never_downgrades_a_newer_minor_or_one_it_cannot_read() {
    for (version, why) in [(json!("0.21.0"), "a newer minor"), (json!("unknown"), "a version that cannot be told")] {
        let world = World::new();
        installed(&world, "0.0.0");
        set_version(&world.dirs, version);
        let host = world.host_pinning(Some(PINNED));
        host.poll();
        let registry = world.registry_at_0_20_2(0.0);
        host.begin_install("again").unwrap().run(&registry, &install_options(&world)).unwrap();
        let asked = registry.asked();
        assert!(
            asked.iter().any(|url| url.ends_with("/sidevoice/latest")),
            "{why}: the latest, not the pin: {asked:?}"
        );
        assert!(!asked.contains(&"https://registry.npmjs.org/sidevoice".to_string()), "{why}: {asked:?}");
    }
}
