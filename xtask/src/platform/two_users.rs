//! `two-users`: the local host with two users, on Linux (it needs `sudo` and `useradd`).

use std::fs;
use std::path::Path;

use crate::platform::file_mode::chmod;
use crate::platform::host::require_os;
use crate::util::{ensure, grep, mkdir, read, repo, require_ci, sleep, Cmd};
use crate::Result;

/// The second user of `two-users`.
const OTHER: &str = "sidevoice-other";

/// `two-users` (Linux, CI only: it adds a user): the local host with two users. `local-host-ci serve` runs as this
/// user (a stand-in core on the socket it publishes, the app's side paired with it, the proxy), `attack` as another, who must get
/// nowhere: it cannot open that socket, link as a connector, or use the proxy
/// (test-support/src/bin/local-host-ci.rs).
pub fn run() -> Result<()> {
    require_os("linux", "two-users")?;
    require_ci("two-users")?;
    let app = repo().join("desktop");
    Cmd::new("cargo")
        .args(["build", "-p", "sidevoice-desktop-test-support", "--bin", "local-host-ci"])
        .dir(&app)
        .run()?;
    // Outside any home, where the second user can reach it.
    let dir = Path::new("/tmp/svci");
    mkdir(dir)?;
    chmod(dir, 0o755)?;
    let tool = dir.join("local-host-ci");
    fs::copy(repo().join("target/debug/local-host-ci"), &tool).map_err(|e| format!("{}: {e}", tool.display()))?;
    chmod(&tool, 0o755)?;
    if !Cmd::new("id").arg(OTHER).attempt()?.0 {
        Cmd::new("sudo").args(["useradd", "--create-home", OTHER]).run()?;
    }
    let (data, app_dir, port_file, log) = (dir.join("d"), dir.join("app"), dir.join("port"), dir.join("serve.log"));
    let _ = fs::remove_file(&port_file);
    let mut serve = Cmd::new(&tool).arg("serve").arg(&data).arg(&app_dir).arg(&port_file).spawn(&log)?;
    for _ in 0..50 {
        if fs::metadata(&port_file).is_ok_and(|m| m.len() > 0) {
            break;
        }
        sleep(0.2);
    }
    let port = fs::read_to_string(&port_file).map_err(|e| format!("serve wrote no port: {e}"))?.trim().to_owned();
    // Where the stand-in core listens: the endpoint it publishes, which the second user cannot read.
    let record =
        fs::read_to_string(data.join("core/core.json")).map_err(|e| format!("serve published no core record: {e}"))?;
    let socket = serde_json::from_str::<serde_json::Value>(&record)
        .ok()
        .and_then(|v| v["endpoint"].as_str().map(str::to_owned))
        .ok_or("core.json names no endpoint")?;
    Cmd::new("ls").arg("-la").arg(dir).arg(&data).run()?;
    eprintln!("--- the second user");
    Cmd::new("sudo")
        .args(["-u", OTHER])
        .arg(&tool)
        .arg("attack")
        .arg(&data)
        .arg(&app_dir)
        .arg(&port)
        .arg(&socket)
        .run()?;
    eprintln!("--- the same attempts as the first user (control: the socket and the files are open to it)");
    let (control, _) = Cmd::new(&tool).arg("attack").arg(&data).arg(&app_dir).arg(&port).arg(&socket).attempt()?;
    ensure(!control, "the control got nowhere")?;
    serve.stop();
    let text = read(&log);
    eprintln!("--- serve\n{text}");
    // The second user's attempts never reached the core: it saw only the first user's requests.
    ensure(grep(&text, "auth=other").is_empty(), "the core saw a foreign token")
}
