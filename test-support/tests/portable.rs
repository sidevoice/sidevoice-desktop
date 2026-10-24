//! The same fixture executable and core listener are exercised on Linux, macOS and Windows.
use serde_json::{json, Value};
use sidevoice_desktop_test_support::{fake_core::FakeCore, fixture};
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
    sidevoice_desktop_test_support::fake_core::forget(&data);
}
