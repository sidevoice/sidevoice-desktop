#![cfg(unix)]
use serde_json::json;
use sidevoice_desktop_test_support::fixture;
use sidevoice_local_host::{cli::Cli, paths::DataDirs, Refusal};
use sidevoice_platform::fs::{Files, TestFiles};
use sidevoice_platform::process::{Process, Processes};
use std::{
    thread,
    time::{Duration, Instant},
};

fn fake(dir: &std::path::Path, config: serde_json::Value) -> Cli {
    let program = dir.join(format!(
        "fake-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    fixture::install(std::path::Path::new(env!("CARGO_BIN_EXE_fake-connector")), &program, &config).unwrap();
    Cli { prefix: vec![program.to_string_lossy().into_owned()], data: dir.to_path_buf() }
}

#[test]
fn the_answer_is_the_json_on_stdout() {
    let tmp = tempfile::tempdir().unwrap();
    let cli = fake(tmp.path(), json!({}));
    let answer = cli.run(&["service", "start", "--json"], Duration::from_secs(5)).unwrap();
    assert_eq!(answer["args"], "service start --json");
    assert_eq!(answer["data"], tmp.path().to_string_lossy().as_ref());
}

#[test]
fn a_refusal_carries_its_key() {
    let tmp = tempfile::tempdir().unwrap();
    let cli =
        fake(tmp.path(), json!({"response":{"ok":false,"error":{"key":"service.not-loaded","message":"m"}},"exit":1}));
    let refusal = cli.run(&["service", "start", "--json"], Duration::from_secs(5)).unwrap_err();
    assert_eq!(refusal, Refusal::new("service.not-loaded", "m"));
}

#[test]
fn no_json_a_crash_or_a_hang_are_failures() {
    let tmp = tempfile::tempdir().unwrap();
    let cli = fake(tmp.path(), json!({"stderr":"boom", "stdout":"", "exit":3}));
    let refusal = cli.run(&["x"], Duration::from_secs(5)).unwrap_err();
    assert_eq!(refusal.key, "cli.failed");
    assert!(refusal.message.contains("boom"), "{}", refusal.message);
    let cli = fake(tmp.path(), json!({"response":{"ok":true},"exit":2}));
    assert_eq!(cli.run(&["x"], Duration::from_secs(5)).unwrap_err().key, "cli.failed");
    let cli = fake(tmp.path(), json!({"delay_ms":30000}));
    let started = Instant::now();
    assert_eq!(cli.run(&["x"], Duration::from_millis(300)).unwrap_err().key, "cli.timeout");
    assert!(started.elapsed() < Duration::from_secs(5));
    let missing = Cli { prefix: vec!["/nonexistent/sidevoice".into()], data: tmp.path().into() };
    assert_eq!(missing.run(&["x"], Duration::from_secs(1)).unwrap_err().key, "cli.failed");
}

/// Whether `pid` is still running (a zombie waiting for its reaper is not).
fn alive(pid: u32) -> bool {
    Processes::identity(pid).is_some()
}

fn dead_soon(pid: u32) -> bool {
    (0..100).any(|_| {
        !alive(pid) || {
            thread::sleep(Duration::from_millis(20));
            false
        }
    })
}

fn pid_in(path: &std::path::Path) -> u32 {
    std::fs::read_to_string(path).unwrap().trim().parse().unwrap()
}

#[test]
fn a_child_left_holding_the_pipes_neither_delays_the_answer_nor_survives() {
    let tmp = tempfile::tempdir().unwrap();
    let pid = tmp.path().join("pid");
    let cli = fake(tmp.path(), json!({"child":true,"pid":pid}));
    let started = Instant::now();
    assert_eq!(cli.run(&["x"], Duration::from_secs(10)).unwrap()["ok"], true);
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    assert!(dead_soon(pid_in(&pid)), "the leftover child was killed");
}

#[test]
fn a_timeout_kills_the_whole_run() {
    let tmp = tempfile::tempdir().unwrap();
    let pid = tmp.path().join("pid");
    let cli = fake(tmp.path(), json!({"child":true,"pid":pid,"delay_ms":30000}));
    let started = Instant::now();
    assert_eq!(cli.run(&["x"], Duration::from_millis(500)).unwrap_err().key, "cli.timeout");
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    assert!(dead_soon(pid_in(&pid)), "the child was killed with the program");
}

#[test]
fn what_the_program_detaches_into_its_own_session_stays() {
    let tmp = tempfile::tempdir().unwrap();
    let pid = tmp.path().join("pid");
    let stop = tmp.path().join("stop");
    let cli = fake(tmp.path(), json!({"detached":true,"pid":pid,"stop":stop}));
    assert_eq!(cli.run(&["service", "start", "--json"], Duration::from_secs(10)).unwrap()["ok"], true);
    let pid = pid_in(&pid);
    thread::sleep(Duration::from_millis(200));
    assert!(alive(pid), "a service the program started on its own stays");
    std::fs::write(&stop, b"").unwrap();
    assert!(dead_soon(pid), "the sleeper ends when told to");
}

#[test]
fn what_runs_is_what_was_checked_even_if_a_link_is_retargeted() {
    let tmp = tempfile::tempdir().unwrap();
    Files::shut_others_out(tmp.path());
    let program = |name: &str| {
        let path = tmp.path().join(name);
        fixture::install(std::path::Path::new(env!("CARGO_BIN_EXE_fake-connector")), &path, &json!({"which":name}))
            .unwrap();
        path
    };
    let (genuine, replacement) = (program("genuine"), program("replacement"));
    let link = tmp.path().join("sidevoice");
    Files::symlink(&genuine, &link);
    let dirs = DataDirs::new(tmp.path());
    std::fs::write(dirs.install_record(), format!(r#"{{"command":["{}"]}}"#, link.display())).unwrap();
    Files::shut_others_out(&dirs.install_record());
    let cli = Cli::installed(&dirs).unwrap();
    // Between the check and the run, the link is pointed elsewhere.
    std::fs::remove_file(&link).unwrap();
    Files::symlink(&replacement, &link);
    let answer = cli.run(&["x"], Duration::from_secs(5)).unwrap();
    assert_eq!(answer["which"], "genuine", "the checked target runs, not the link's new one");
    let canonical_data = std::fs::canonicalize(tmp.path()).unwrap();
    assert_eq!(answer["data"], canonical_data.display().to_string(), "SIDEVOICE_DATA_DIR is the checked directory");
}

#[test]
fn only_an_install_record_of_absolute_paths_names_the_cli() {
    let tmp = tempfile::tempdir().unwrap();
    Files::shut_others_out(tmp.path());
    let dirs = DataDirs::new(tmp.path());
    assert_eq!(Cli::installed(&dirs).unwrap_err().key, "cli.unavailable");
    let write = |text: &str, loose: bool| {
        std::fs::write(dirs.install_record(), text).unwrap();
        Files::shut_others_out(&dirs.install_record());
        if loose {
            Files::let_others_change(&dirs.install_record());
        }
    };
    let script = tmp.path().join("cli.mjs");
    std::fs::write(&script, b"").unwrap();
    let executable = env!("CARGO_BIN_EXE_fake-connector");
    let record = json!({"command":[executable, script]}).to_string();
    write(&record, false);
    let cli = Cli::installed(&dirs).unwrap();
    let canonical = |p: &str| std::fs::canonicalize(p).unwrap().display().to_string();
    assert_eq!(cli.prefix, [canonical(executable), canonical(&script.display().to_string())]);
    // What the command names must be root's or this user's and closed to others too.
    Files::let_others_change(&script);
    assert_eq!(Cli::installed(&dirs).unwrap_err().key, "install.unsafe");
    Files::shut_others_out(&script);
    Files::let_others_read(&script);
    write(r#"{"command":["/nonexistent/node"]}"#, false);
    assert_eq!(Cli::installed(&dirs).unwrap_err().key, "cli.unavailable");
    for bad in [r#"{"command":["node","cli.mjs"]}"#, r#"{"command":[]}"#, r#"{"command":"/bin/sh"}"#, "{}", "nope"] {
        write(bad, false);
        let key = Cli::installed(&dirs).unwrap_err().key;
        assert_eq!(key, "install.unreadable", "{bad}");
    }
    write(&json!({"command":[executable]}).to_string(), true);
    assert_eq!(Cli::installed(&dirs).unwrap_err().key, "install.unsafe");
    write(&json!({"command":[executable]}).to_string(), false);
    Files::let_others_change(tmp.path());
    assert_eq!(Cli::installed(&dirs).unwrap_err().key, "install.unsafe");
    Files::shut_others_out(tmp.path());
}
