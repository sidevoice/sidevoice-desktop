use serde_json::json;
use sidevoice_desktop_test_support::fixture;
use sidevoice_desktop_test_support::platform::storage::{StorageFixture, StorageFixtures};
use sidevoice_local_host::{cli::Cli, paths::DataDirs, Refusal};
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
fn the_program_runs_in_its_own_directory_not_the_apps() {
    let tmp = tempfile::tempdir().unwrap();
    let cli = fake(tmp.path(), json!({}));
    let answer = cli.run(&["x"], Duration::from_secs(5)).unwrap();
    let cwd = std::path::PathBuf::from(answer["cwd"].as_str().expect("the fake says where it ran"));
    let program_dir = std::path::Path::new(&cli.prefix[0]).parent().unwrap();
    assert_eq!(cwd.canonicalize().unwrap(), program_dir.canonicalize().unwrap());
    assert_ne!(cwd.canonicalize().unwrap(), std::env::current_dir().unwrap().canonicalize().unwrap());
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
    StorageFixtures::private(tmp.path());
    let program = |name: &str| {
        let path = tmp.path().join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        fixture::install(std::path::Path::new(env!("CARGO_BIN_EXE_fake-connector")), &path, &json!({"which":name}))
            .unwrap();
        path
    };
    let (genuine, replacement) = (program("genuine"), program("replacement"));
    let link = tmp.path().join(fixture::program_name());
    StorageFixtures::symlink(&genuine, &link);
    let dirs = DataDirs::new(tmp.path());
    std::fs::write(dirs.install_record(), json!({"command": [link]}).to_string()).unwrap();
    StorageFixtures::private(&dirs.install_record());
    let cli = Cli::installed(&dirs).unwrap();
    // Between the check and the run, the link is pointed elsewhere.
    std::fs::remove_file(&link).unwrap();
    StorageFixtures::symlink(&replacement, &link);
    let answer = cli.run(&["x"], Duration::from_secs(5)).unwrap();
    assert_eq!(answer["which"], "genuine", "the checked target runs, not the link's new one");
    let canonical_data = std::fs::canonicalize(tmp.path()).unwrap();
    assert_eq!(answer["data"], canonical_data.display().to_string(), "SIDEVOICE_DATA_DIR is the checked directory");
}

#[test]
fn only_an_install_record_of_absolute_paths_names_the_cli() {
    let tmp = tempfile::tempdir().unwrap();
    StorageFixtures::private(tmp.path());
    let dirs = DataDirs::new(tmp.path());
    assert_eq!(Cli::installed(&dirs).unwrap_err().key, "cli.unavailable");
    let write = |text: &str, writable: bool| {
        std::fs::write(dirs.install_record(), text).unwrap();
        StorageFixtures::private(&dirs.install_record());
        if writable {
            StorageFixtures::writable(&dirs.install_record());
        }
    };
    let executable = tmp.path().join(fixture::program_name());
    let bytes = std::fs::read(env!("CARGO_BIN_EXE_fake-connector")).unwrap();
    std::fs::write(&executable, &bytes).unwrap();
    fixture::configure(&executable, &json!({})).unwrap();
    let record = json!({"command":[executable]}).to_string();
    write(&record, false);
    // Native applications must be executable where that is a permission (Windows: the `.exe` name is what runs);
    // discovery never changes their contents or permissions.
    if StorageFixtures::EXECUTION_IS_A_PERMISSION {
        assert_eq!(Cli::installed(&dirs).unwrap_err().key, "install.unsafe");
    }
    assert_eq!(std::fs::read(&executable).unwrap(), bytes);
    StorageFixtures::assert_not_executable(&executable);
    StorageFixtures::executable(&executable);
    let cli = Cli::installed(&dirs).unwrap();
    assert_eq!(cli.prefix, [std::fs::canonicalize(&executable).unwrap().display().to_string()]);
    let answer = cli.run(&["service", "start", "--json"], Duration::from_secs(5)).unwrap();
    assert_eq!(answer["args"], "service start --json");
    // The recorded native application must remain closed to writes by others.
    StorageFixtures::writable(&executable);
    assert_eq!(Cli::installed(&dirs).unwrap_err().key, "install.unsafe");
    StorageFixtures::private(&executable);
    let missing = tmp.path().join("missing").join(fixture::program_name());
    write(&json!({"command":[missing]}).to_string(), false);
    assert_eq!(Cli::installed(&dirs).unwrap_err().key, "cli.unavailable");
    for bad in [
        json!({"command":[fixture::program_name()]}).to_string(),
        json!({"command":[]}).to_string(),
        json!({"command":executable}).to_string(),
        "{}".into(),
        "nope".into(),
    ] {
        write(&bad, false);
        let key = Cli::installed(&dirs).unwrap_err().key;
        assert_eq!(key, "install.unreadable", "{bad}");
    }
    write(&json!({"command":[executable]}).to_string(), true);
    assert_eq!(Cli::installed(&dirs).unwrap_err().key, "install.unsafe");
    write(&json!({"command":[executable]}).to_string(), false);
    StorageFixtures::writable(tmp.path());
    assert_eq!(Cli::installed(&dirs).unwrap_err().key, "install.unsafe");
    StorageFixtures::private(tmp.path());
}
