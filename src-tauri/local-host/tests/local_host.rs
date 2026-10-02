//! The local host end to end on this machine: a stand-in core on a real Unix socket (HTTP and a WebSocket echo), the
//! app's pairing with it and the startup cases (design §4.1), the proxy a page talks to — forwarding with the token
//! swapped in, a socket spliced both ways, the refusals — and the service's state and actions through a stand-in
//! connector and CLI.

use serde_json::{json, Value};
use sidevoice_local_host::cli::Cli;
use sidevoice_local_host::fake_core::{self, FakeCore};
use sidevoice_local_host::host::{Action, Config, LocalHost};
use sidevoice_local_host::http;
use sidevoice_local_host::paths::DataDirs;
use sidevoice_local_host::state::State;
use sidevoice_local_host::store;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct World {
    _tmp: tempfile::TempDir,
    dirs: DataDirs,
    app_dir: PathBuf,
    log: Arc<Mutex<Vec<String>>>,
}

impl World {
    fn new() -> World {
        // Under /tmp: a socket's path must stay short.
        let tmp = tempfile::Builder::new().prefix("svlh").tempdir_in("/tmp").unwrap();
        let dirs = DataDirs::new(tmp.path().join("d"));
        let app_dir = tmp.path().join("app");
        World { _tmp: tmp, dirs, app_dir, log: Arc::default() }
    }

    fn core(&self, seed: u8) -> Arc<FakeCore> {
        FakeCore::start(&self.dirs, seed, false).unwrap()
    }

    fn host(&self) -> Arc<LocalHost> {
        let config = Config { dirs: self.dirs.clone(), app_dir: self.app_dir.clone(), name: "test-mac".into() };
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

fn bundled_fixture(world: &World, body: &str) -> Cli {
    let executable = world._tmp.path().join("bundled-sidevoice-fixture");
    std::fs::write(&executable, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    Cli { prefix: vec![executable.to_string_lossy().into_owned()], data: world.dirs.data.clone() }
}

fn install_script(delay: &str, progress: &str) -> String {
    format!(
        r#"case "$1:$2" in
  install:*)
    {progress}
    {delay}
    printf '{{"command":["%s"]}}\n' "$0" > "$SIDEVOICE_DATA_DIR/install.json"
    echo '{{"ok":true,"result":"installed"}}'
    ;;
  service:status)
    echo '{{"ok":true,"state":"running","service":"launchd","calls":0}}'
    ;;
  *) echo '{{"ok":false,"error":{{"key":"fixture.bad-request","message":"bad request"}}}}'; exit 1 ;;
esac"#,
        progress = progress,
        delay = delay,
    )
}

#[test]
fn bundled_install_has_one_job_progress_and_a_commit_cancellation_boundary() {
    let world = World::new();
    let _core = world.core(1);
    let host = world.host();
    let progress = r#"echo '{"type":"progress","step":"download","done":5,"total":10}' >&2
echo '{"type":"progress","step":"commit","done":null,"total":null}' >&2"#;
    let cli = bundled_fixture(&world, &install_script("sleep 0.6", progress));
    let worker_host = host.clone();
    let worker_cli = cli.clone();
    let worker = std::thread::spawn(move || worker_host.install_bundled(worker_cli, "job-a".into(), false));

    let mut frames = Vec::new();
    for _ in 0..30 {
        if let Ok(next) = host.install_progress("job-a", 0) {
            if !next.is_empty() {
                frames = next;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(host.state().state, State::Installing);
    assert_eq!(frames.iter().map(|frame| frame.step.as_str()).collect::<Vec<_>>(), ["download", "commit"]);
    assert_eq!((frames[0].done, frames[0].total), (Some(5), Some(10)));
    assert!(!frames[1].cancellable, "commit is the cancellation boundary");
    assert!(!host.cancel_install("job-a"), "a committed transaction is allowed to finish");
    assert_eq!(host.install_bundled(cli, "job-b".into(), false).unwrap_err().key, "install.busy");

    let report = worker.join().unwrap().unwrap();
    assert_eq!(report.state, State::Running);
    assert!(report.reachable);
    assert!(host.pairing().is_some(), "the existing R1 poll projects the local pairing");
    assert_eq!(host.install_progress("job-a", 0).unwrap_err().key, "install.job-ended");
}

#[test]
fn bundled_install_cancellation_is_acknowledged_as_a_keyed_connector_result() {
    let world = World::new();
    let _core = world.core(1);
    let host = world.host();
    let executable = world._tmp.path().join("cancellable-sidevoice-fixture");
    let body = r#"#!/usr/bin/perl
$| = 1;
if ($ARGV[0] eq 'install') {
  $SIG{INT} = sub { print qq({"ok":false,"error":{"key":"install.cancelled","message":"cancelled"}}\n); exit 1; };
  print STDERR qq({"type":"progress","step":"stage","done":null,"total":null}\n);
  sleep 30;
} elsif ($ARGV[0] eq 'service' && $ARGV[1] eq 'status') {
  print qq({"ok":true,"state":"running","service":"launchd","calls":0}\n);
} else {
  print qq({"ok":false,"error":{"key":"fixture.bad-request","message":"bad request"}}\n);
  exit 1;
}
"#;
    std::fs::write(&executable, body).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let cli = Cli { prefix: vec![executable.to_string_lossy().into_owned()], data: world.dirs.data.clone() };
    let worker_host = host.clone();
    let worker = std::thread::spawn(move || worker_host.install_bundled(cli, "job-cancel".into(), false));
    for _ in 0..30 {
        if host.install_progress("job-cancel", 0).is_ok_and(|frames| !frames.is_empty()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(host.cancel_install("job-cancel"), "true follows the connector's install.cancelled response");
    assert_eq!(worker.join().unwrap().unwrap_err().key, "install.cancelled");
    assert!(!world.dirs.install_record().exists(), "the fixture did not select an install after cancellation");
}

#[test]
fn bundled_update_keeps_the_connector_rollback_error_even_when_the_old_core_is_live() {
    let world = World::new();
    let _core = world.core(1);
    let host = world.host();
    assert_eq!(host.poll().state, State::Running);
    let cli = bundled_fixture(
        &world,
        r#"if [ "$1" = install ]; then
  echo '{"type":"progress","step":"rollback","done":null,"total":null}' >&2
  echo '{"ok":false,"error":{"key":"install.rollback","message":"the previous release was restored"}}'
  exit 1
fi
echo '{"ok":false,"error":{"key":"fixture.unexpected","message":"unexpected command"}}'
exit 1"#,
    );

    let failure = host.install_bundled(cli, "job-rollback".into(), true).unwrap_err();
    assert_eq!(failure.key, "install.rollback");
    assert_eq!(failure.message, "The connector rolled back the installation after a failure.");
    assert_eq!(host.state().state, State::Running, "the selected old core remains usable after rollback");
}

#[test]
fn pairs_over_the_socket_and_the_page_gets_only_the_secret() {
    let world = World::new();
    let core = world.core(1);
    let host = world.host();
    let (port, secret) = running(&host);

    let stored = store::load(&world.app_dir).expect("local-host.json");
    let mode = std::fs::metadata(world.app_dir.join(store::FILE_NAME)).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(stored.fp, core.fingerprint());
    assert_eq!(core.local_device().as_deref(), Some(stored.device_id.as_str()));
    assert_eq!(
        host.pairing().unwrap(),
        json!({
            "fp": core.fingerprint(), "public_key": core.public_key(), "device_id": stored.device_id,
            "token": secret, "urls": [format!("http://127.0.0.1:{port}")], "rv": null, "host": "fake-machine",
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
    std::fs::set_permissions(world.dirs.core(), std::fs::Permissions::from_mode(0o750)).unwrap();
    let host = world.host();
    let report = serde_json::to_value(host.poll()).unwrap();
    assert_eq!(report["state"], "failed");
    assert_eq!(report["failure"]["key"], "identity.unsafe-directory");
    assert!(core.requests().is_empty(), "{:?}", core.requests());
    std::fs::set_permissions(world.dirs.core(), std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn with_nothing_installed_the_host_is_absent() {
    let world = World::new();
    let host = world.host();
    assert_eq!(serde_json::to_value(host.poll()).unwrap(), json!({"state": "absent", "reachable": false}));
    assert_eq!(host.act(Action::Start).unwrap_err().key, "cli.unavailable");
}

/// A stand-in CLI recorded in `install.json`: it appends its arguments to `calls.log` and answers per subcommand.
fn install_fake_cli(dirs: &DataDirs, status: &str) -> PathBuf {
    std::fs::create_dir_all(&dirs.data).unwrap();
    std::fs::set_permissions(&dirs.data, std::fs::Permissions::from_mode(0o700)).unwrap();
    let script = dirs.data.join("fake-cli");
    let log = dirs.data.join("calls.log");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{log}'\ncase \"$1 $2\" in\n\
             'service status') echo '{status}' ;;\n\
             'pair-device --json') echo '{{\"ok\":true,\"code\":\"SV1-CODE\",\"expires_in\":600,\"reach\":\"local-only\",\"payload\":{{}}}}' ;;\n\
             pair*) echo '{{\"ok\":true,\"room\":\"https://room.example\",\"connector_id\":\"c1\"}}' ;;\n\
             'service start') echo '{{\"ok\":false,\"error\":{{\"key\":\"service.not-loaded\",\"message\":\"m\"}}}}'; exit 1 ;;\n\
             *) echo '{{\"ok\":true,\"state\":\"starting\",\"service\":\"launchd\"}}' ;;\nesac\n",
            log = log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(dirs.install_record(), json!({"command": [script]}).to_string()).unwrap();
    std::fs::set_permissions(dirs.install_record(), std::fs::Permissions::from_mode(0o600)).unwrap();
    log
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
        json!({"state": "stopped-by-person", "service": "launchd", "reachable": false}),
        "calls null: unknown"
    );
    host.poll();
    assert_eq!(calls(&log), ["service status --json"], "the fallback is not re-run on every poll");

    assert_eq!(host.act(Action::Restart).unwrap().state, State::StoppedByPerson);
    let refused = host.act(Action::Start).unwrap_err();
    assert_eq!(refused.key, "service.not-loaded");
    host.act(Action::Stop).unwrap();
    host.act(Action::ServiceInstall).unwrap();
    host.act(Action::ServiceUninstall).unwrap();
    assert_eq!(host.pairing_code().unwrap(), json!({"code": "SV1-CODE", "expires_in": 600, "reach": "local-only"}));
    assert_eq!(host.pair_room("https://room.example", "ROOM-CODE").unwrap(), json!({"room": "https://room.example"}));
    for (url, code) in
        [("-x", "c"), ("ftp://a", "c"), ("https://a", "--force"), ("https://a b", "c"), ("https://a", "")]
    {
        assert_eq!(host.pair_room(url, code).unwrap_err().key, "bad_request", "{url} {code}");
    }
    let ran = calls(&log);
    for expected in [
        "service restart --json",
        "service start --json",
        "service stop --json",
        "service install --json",
        "service uninstall --json",
        "pair-device --json",
        "pair https://room.example ROOM-CODE --json",
    ] {
        assert!(ran.iter().any(|c| c == expected), "{expected} not in {ran:?}");
    }
    assert!(!ran.iter().any(|c| c.contains("--force") || c.starts_with("pair -x")));
}

#[test]
fn a_connector_that_answers_is_asked_first() {
    let world = World::new();
    let log = install_fake_cli(&world.dirs, r#"{"ok":true,"state":"absent"}"#);
    let listener = UnixListener::bind(world.dirs.connector_socket()).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut line = String::new();
            let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(&stream), &mut line);
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
    assert!(calls(&log).is_empty(), "no CLI while the connector answers");
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
    let listener = UnixListener::bind(world.dirs.connector_socket()).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut line = String::new();
            let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(&stream), &mut line);
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
    assert!(calls(&log).is_empty(), "no CLI while a connector answers");
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
