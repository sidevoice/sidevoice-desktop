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
/// `CARGO_BIN_EXE_fake-connector`: built by the workspace's tests (`cargo test --workspace`) into the profile directory
/// that holds the running test binary's `deps`.
pub fn fake_connector() -> PathBuf {
    let test = std::env::current_exe().expect("the running test binary");
    let profile = test.parent().and_then(Path::parent).expect("a test binary under <target>/<profile>/deps");
    let program = profile.join(format!("fake-connector{}", std::env::consts::EXE_SUFFIX));
    assert!(
        program.is_file(),
        "{} is missing: the workspace's tests (`cargo test --workspace`) build it",
        program.display()
    );
    program
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
