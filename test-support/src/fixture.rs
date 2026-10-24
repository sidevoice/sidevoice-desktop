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
