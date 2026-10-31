//! The same fixture executable and core listener are exercised on Linux, macOS and Windows.
use serde_json::{json, Value};
use sidevoice_desktop_test_support::platform::endpoint_cleanup;
use sidevoice_desktop_test_support::platform::registry::RegistryTransport;
use sidevoice_desktop_test_support::{fake_core::FakeCore, fake_registry::FakeRegistry, fixture};
use sidevoice_platform::ipc::{Channel, Channels, Endpoint};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn native(dir: &Path, config: Value) -> PathBuf {
    let program = dir.join(fixture::program_name());
    fixture::install(Path::new(env!("CARGO_BIN_EXE_fake-connector")), &program, &config).unwrap();
    program
}

#[test]
fn native_cli_answers_and_refuses_without_a_shell() {
    let tmp = tempfile::tempdir().unwrap();
    let program = native(tmp.path(), json!({}));
    let answer = Command::new(&program)
        .args(["service", "start", "--json"])
        .env("SIDEVOICE_DATA_DIR", tmp.path())
        .output()
        .unwrap();
    assert!(answer.status.success());
    let value: Value = serde_json::from_slice(&answer.stdout).unwrap();
    assert_eq!(value["args"], "service start --json");
    assert_eq!(value["data"], json!(tmp.path()));
    fixture::configure(
        &program,
        &json!({"response":{"ok":false,"error":{"key":"service.not-loaded"}},"stderr":"refused","exit":1}),
    )
    .unwrap();
    let answer = Command::new(&program).output().unwrap();
    assert!(!answer.status.success());
    assert_eq!(serde_json::from_slice::<Value>(&answer.stdout).unwrap()["error"]["key"], "service.not-loaded");
    assert!(String::from_utf8_lossy(&answer.stderr).contains("refused"));
}

#[test]
fn native_cli_can_delay_its_exit() {
    let tmp = tempfile::tempdir().unwrap();
    let program = native(tmp.path(), json!({"delay_ms":200}));
    let start = std::time::Instant::now();
    assert!(Command::new(program).output().unwrap().status.success());
    assert!(start.elapsed() >= std::time::Duration::from_millis(200));
}

#[test]
fn registry_packages_the_native_executable_and_configuration() {
    let config = json!({"response":{"ok":true,"action":"installed"}});
    let executable = Path::new(env!("CARGO_BIN_EXE_fake-connector"));
    let registry = fixture::release(executable, "0.8.1", "@sidevoice/sidevoice-fixture", &config);
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(registry.get(&registry.tarball_url()).body));
    let tmp = tempfile::tempdir().unwrap();
    archive.unpack(tmp.path()).unwrap();
    let program = tmp.path().join("package/bin").join(fixture::program_name());
    assert_eq!(std::fs::read(&program).unwrap(), std::fs::read(executable).unwrap());
    let answer = Command::new(program).args(["install", "--json"]).output().unwrap();
    assert!(answer.status.success());
    assert_eq!(serde_json::from_slice::<Value>(&answer.stdout).unwrap(), config["response"]);
}

#[test]
fn registry_transport_preserves_missing_and_malformed_responses() {
    let registry = FakeRegistry::release("0.8.1", "@sidevoice/sidevoice-fixture", &[]);
    let missing = registry.get("https://registry.npmjs.org/missing");
    assert_eq!((missing.status, missing.length), (404, None));

    let url = registry.tarball_url();
    registry.serve_with_length(&url, b"short".to_vec(), 99);
    let mut response = registry.get(&url);
    let mut body = Vec::new();
    std::io::Read::read_to_end(&mut response.body, &mut body).unwrap();
    assert_eq!((response.status, response.length, body), (200, Some(99), b"short".to_vec()));
    assert_eq!(registry.asked(), ["https://registry.npmjs.org/missing", url.as_str()]);
}

#[test]
fn fake_core_publishes_and_serves_the_shared_channel() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("d");
    let core = FakeCore::start(&data, 1, false).unwrap();
    let record: Value = serde_json::from_slice(&std::fs::read(data.join("core/core.json")).unwrap()).unwrap();
    let endpoint: Endpoint = record["endpoint"].as_str().unwrap().parse().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let mut stream = Channels::connect(&endpoint).await.unwrap();
        stream.write_all(b"GET /api/local/health HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
        let mut answer = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(5), stream.read_to_end(&mut answer))
            .await
            .unwrap()
            .unwrap();
        let body = answer.windows(4).position(|b| b == b"\r\n\r\n").unwrap() + 4;
        let health: Value = serde_json::from_slice(&answer[body..]).unwrap();
        assert_eq!(health["fingerprint"], core.fingerprint());
        assert_eq!(health["public_key"], core.public_key());
    });
    core.stop();
    assert!(!data.join("core/core.json").exists());
    endpoint_cleanup::forget(&data);
}

#[test]
fn closing_an_older_call_keeps_the_newer_call_open() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{
        tungstenite::{protocol::Role, Message},
        WebSocketStream,
    };

    async fn open_call(endpoint: &Endpoint, token: &str) -> WebSocketStream<sidevoice_platform::ipc::Stream> {
        let mut stream = Channels::connect(endpoint).await.unwrap();
        stream
            .write_all(
                format!(
            "GET /api/browser/call HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Protocol: sidevoice, sidevoice.token.{token}\r\n\r\n"
        )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(stream.read_u8().await.unwrap());
        }
        assert!(head.starts_with(b"HTTP/1.1 101 "));
        WebSocketStream::from_raw_socket(stream, Role::Client, None).await
    }

    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("d");
    let core = FakeCore::start(&data, 1, false).unwrap();
    let record: Value = serde_json::from_slice(&std::fs::read(data.join("core/core.json")).unwrap()).unwrap();
    let endpoint: Endpoint = record["endpoint"].as_str().unwrap().parse().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let result = runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut pairing = Channels::connect(&endpoint).await.unwrap();
            pairing
                .write_all(b"POST /api/device/local/pair HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
            let mut response = Vec::new();
            pairing.read_to_end(&mut response).await.unwrap();
            let body = response.windows(4).position(|b| b == b"\r\n\r\n").unwrap() + 4;
            let paired: Value = serde_json::from_slice(&response[body..]).unwrap();
            let token = paired["token"].as_str().unwrap();

            let mut a = open_call(&endpoint, token).await;
            let mut b = open_call(&endpoint, token).await;
            b.send(Message::Text("before A closes".into())).await.unwrap();
            assert_eq!(b.next().await.unwrap().unwrap(), Message::Text("before A closes".into()));

            a.close(None).await.unwrap();
            // Wait for EOF so A's server handler has finished its cleanup before exercising B.
            a.get_mut().read_to_end(&mut Vec::new()).await.unwrap();
            b.send(Message::Text("after A closes".into())).await.unwrap();
            let echoed = b.next().await.unwrap();
            core.revoke_all();
            assert_eq!(echoed.unwrap(), Message::Text("after A closes".into()));
            assert!(b.next().await.is_none_or(|message| message.is_err()), "revocation still closes B");
        })
        .await
    });
    core.stop();
    endpoint_cleanup::forget(&data);
    result.unwrap();
}

// Exercise the shell's actual handoff protocol in the existing Windows/macOS fixture jobs,
// without linking a webview. Tests remain colocated with the desktop platform interface.
#[path = "../../desktop/src/platform/reset_handoff.rs"]
mod reset_handoff;

// Exercise the actual shell pairing store without linking the native UI.
#[path = "../../desktop/src/platform/pairing_storage.rs"]
mod pairing_storage;
mod platform {
    pub(crate) use crate::pairing_storage;
}
#[path = "../../desktop/src/data/pairings.rs"]
mod pairings;
