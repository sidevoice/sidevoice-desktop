//! A native executable plus a JSON sidecar describes a fake CLI, including inside a registry tarball.
use serde_json::Value;
use sidevoice_platform::fs::{Create, Files, Fs};
use std::io::Write;
use std::{
    io,
    path::{Path, PathBuf},
};

pub fn program_name() -> String {
    format!("sidevoice-connector{}", std::env::consts::EXE_SUFFIX)
}
pub fn config_path(program: &Path) -> PathBuf {
    program.with_extension("fixture.json")
}
/// The native fake connector (`src/bin/fake-connector.rs`) for tests of other packages, which have no
/// `CARGO_BIN_EXE_fake-connector`: built from this checkout's sources by the Cargo running the tests (a no-op when it
/// is fresh) the first time it is asked for, and the path Cargo reports for it. Never a missing, stale or other
/// checkout's binary.
pub fn fake_connector() -> PathBuf {
    static BUILT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BUILT.get_or_init(build_fake_connector).clone()
}
fn build_fake_connector() -> PathBuf {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| env!("CARGO").into());
    let built = std::process::Command::new(cargo)
        .args(["build", "--quiet", "--message-format=json", "--bin", "fake-connector", "--manifest-path"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .stderr(std::process::Stdio::inherit())
        .output()
        .expect("Cargo runs");
    assert!(built.status.success(), "the fake connector does not build");
    let executable = String::from_utf8_lossy(&built.stdout).lines().find_map(|line| {
        let message: Value = serde_json::from_str(line).ok()?;
        (message["reason"] == "compiler-artifact" && message["target"]["name"] == "fake-connector")
            .then(|| message["executable"].as_str().map(PathBuf::from))
            .flatten()
    });
    executable.expect("Cargo names the fake connector it built")
}
pub fn install(executable: &Path, program: &Path, config: &Value) -> io::Result<()> {
    Files::open_private(program, Create::NewProgram)?.write_all(&std::fs::read(executable)?)?;
    configure(program, config)
}
pub fn configure(program: &Path, config: &Value) -> io::Result<()> {
    std::fs::write(config_path(program), serde_json::to_vec(config)?)
}
pub fn release(executable: &Path, version: &str, platform: &str, config: &Value) -> crate::fake_registry::FakeRegistry {
    let program = format!("bin/{}", program_name());
    let sidecar = config_path(Path::new(&program));
    crate::fake_registry::FakeRegistry::release(
        version,
        platform,
        &[
            (&program, &std::fs::read(executable).expect("prebuilt fixture"), 0o755),
            (sidecar.to_str().unwrap(), &serde_json::to_vec(config).unwrap(), 0o644),
            ("connector.json", b"{}", 0o644),
        ],
    )
}
