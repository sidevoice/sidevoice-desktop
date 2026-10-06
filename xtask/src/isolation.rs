//! `isolation` (Linux, CI): the local host with two users. `local-host-ci serve` runs as this user (a stand-in core
//! on `local.sock`, the app's side paired with it, the proxy); `attack` runs as a second user, who must get nowhere,
//! and again as this user, the control, to whom the socket and the files are open
//! (src-tauri/local-host/examples/local-host-ci.rs).

use crate::util::{has, output, read, repo, require_ci, run, sleep, target, Background};
use crate::Result;
use std::fs;
use std::path::Path;
use std::process::Command;

const OTHER: &str = "sidevoice-other";

pub fn isolation() -> Result<()> {
    require_ci("cargo xtask isolation")?;
    run(Command::new("cargo")
        .args(["build", "--locked", "-p", "sidevoice-local-host", "--example", "local-host-ci"])
        .current_dir(repo()))?;
    // Outside any home, readable by the second user.
    let dir = Path::new("/tmp/svci");
    crate::util::fresh_dir(dir)?;
    run(Command::new("chmod").arg("755").arg(dir))?;
    let tool = dir.join("local-host-ci");
    fs::copy(target().join("debug/examples/local-host-ci"), &tool).map_err(|e| e.to_string())?;
    run(Command::new("chmod").arg("755").arg(&tool))?;
    if Command::new("id").arg(OTHER).output().map_or(true, |o| !o.status.success()) {
        run(Command::new("sudo").args(["useradd", "--create-home", OTHER]))?;
    }
    let (data, app, port_file) = (dir.join("d"), dir.join("app"), dir.join("port"));
    let log = target().join("xtask/serve.log");
    fs::create_dir_all(log.parent().expect("has a parent")).map_err(|e| e.to_string())?;
    let mut serve = Background::start(Command::new(&tool).arg("serve").arg(&data).arg(&app).arg(&port_file), &log)?;
    for _ in 0..50 {
        if fs::metadata(&port_file).is_ok_and(|m| m.len() > 0) {
            break;
        }
        sleep(0.2);
    }
    let port = fs::read_to_string(&port_file).map_err(|e| format!("serve wrote no port: {e}"))?.trim().to_owned();
    eprintln!("{}", output(Command::new("ls").arg("-la").arg(dir).arg(&data))?);

    eprintln!("--- the second user");
    let other = run(Command::new("sudo").args(["-u", OTHER]).arg(&tool).arg("attack").arg(&data).arg(&app).arg(&port));
    eprintln!("--- the same attempts as this user (control: the socket and the files are open to it)");
    let control = Command::new(&tool).arg("attack").arg(&data).arg(&app).arg(&port).status();
    serve.stop();
    eprintln!("--- serve\n{}", read(&log));
    other?;
    if control.is_ok_and(|status| status.success()) {
        return Err("the control got nowhere: the attempts prove nothing".into());
    }
    // The second user's attempts never reached the core: it saw only this user's requests.
    if has(&read(&log), "auth=other") {
        return Err("the core saw a foreign token".into());
    }
    Ok(())
}
