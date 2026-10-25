#![cfg(unix)]
use serde_json::json;
use sidevoice_desktop_test_support::fake_registry::FakeRegistry;
use sidevoice_desktop_test_support::fixture;
use sidevoice_local_host::install::*;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const PLATFORM: &str = "@sidevoice/sidevoice-linux-x64";

struct World {
    tmp: tempfile::TempDir,
    data: PathBuf,
    temp: PathBuf,
}

impl World {
    fn new() -> World {
        let tmp = tempfile::Builder::new().prefix("svin").tempdir_in("/tmp").unwrap();
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let data = tmp.path().join("d");
        let temp = tmp.path().join("t");
        std::fs::create_dir(&temp).unwrap();
        World { tmp, data, temp }
    }

    fn options(&self) -> Options {
        Options {
            package: Some(PLATFORM.into()),
            temp: self.temp.clone(),
            installer_time: Duration::from_secs(20),
            settle_time: Duration::from_millis(100),
            ..Options::standard()
        }
    }

    /// What the stand-in installer left: its arguments and environment, if it ran.
    fn ran(&self) -> Option<String> {
        std::fs::read_to_string(self.tmp.path().join("ran")).ok()
    }

    fn leftovers(&self) -> usize {
        std::fs::read_dir(&self.temp).unwrap().count()
    }

    fn installer(&self, config: serde_json::Value) -> serde_json::Value {
        let mut config = config;
        config["ran"] = json!(self.tmp.path().join("ran"));
        config["install_record"] = json!(true);
        config
    }
    fn registry(&self, config: &serde_json::Value) -> FakeRegistry {
        fixture::release(Path::new(env!("CARGO_BIN_EXE_fake-connector")), "0.8.1", PLATFORM, config)
    }
}

#[test]
fn a_release_is_fetched_verified_unpacked_and_installed_by_its_own_installer() {
    let world = World::new();
    let registry = world.registry(&world.installer(
        json!({"stderr":"staging done","delay_ms":300,"response":{"ok":true,"action":"installed","version":"0.8.1"}}),
    ));
    let job = Job::new("j1");
    let answer = prepare(&job, &registry, &world.data, &world.options()).unwrap();
    assert_eq!(answer["action"], "installed");
    let ran = world.ran().expect("the installer ran");
    assert!(ran.starts_with(&format!("install --json {}", world.data.display())), "{ran}");
    assert_eq!(job.step(), Step::ServiceStart, "install.json written: on to the service");
    assert!(!job.progress().cancellable);
    assert_eq!(world.leftovers(), 0, "the private directory is gone");
    assert_eq!(
        registry.asked(),
        [
            "https://registry.npmjs.org/sidevoice/latest".to_string(),
            format!("https://registry.npmjs.org/{PLATFORM}/0.8.1"),
            registry.tarball_url(),
        ]
    );
}

#[test]
fn a_tarball_that_is_not_the_one_described_is_never_unpacked() {
    let world = World::new();
    let registry = world.registry(&world.installer(json!({"response":{"ok":true}})));
    let mut tampered = registry.tarball_bytes();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    registry.serve(&registry.tarball_url(), 200, tampered);
    let job = Job::new("j");
    let failure = prepare(&job, &registry, &world.data, &world.options()).unwrap_err();
    assert_eq!((failure.key.as_str(), failure.step), ("install.checksum", Some(Step::Verification)));
    assert_eq!(world.ran(), None, "nothing of it ran");
    assert_eq!(world.leftovers(), 0);
}

#[test]
fn a_package_without_its_installer_has_no_bundle() {
    let world = World::new();
    let registry = FakeRegistry::release("0.8.1", PLATFORM, &[("connector.json", b"{}", 0o644)]);
    let failure = prepare(&Job::new("j"), &registry, &world.data, &world.options()).unwrap_err();
    assert_eq!((failure.key.as_str(), failure.step), ("install.no-bundle", Some(Step::Staging)));
    let no_platform = Options { package: None, ..world.options() };
    let failure = prepare(&Job::new("j"), &registry, &world.data, &no_platform).unwrap_err();
    assert_eq!(failure.key, "install.no-bundle");
    assert!(registry.asked().len() == 3, "a system with no package asks nothing");
}

#[test]
fn the_installers_keyed_failures_reach_the_page_under_the_keys_it_knows() {
    let world = World::new();
    let rolled_back = json!({"stderr":"core: bind failed","exit":1,"response":{"ok":false,"error":{"key":"install.rolled-back","params":{"version":"0.8.1","previous":"0.8.0","cause":"exited"},"message":"Sidevoice 0.8.1 did not start (exited), so this computer is back on 0.8.0."}}});
    let registry = world.registry(&world.installer(rolled_back));
    let failure = prepare(&Job::new("j"), &registry, &world.data, &world.options()).unwrap_err();
    assert_eq!((failure.key.as_str(), failure.step), ("install.rollback", Some(Step::ServiceStart)));
    assert_eq!(failure.params.as_ref().unwrap()["previous"], "0.8.0");
    assert!(failure.message.contains("back on 0.8.0"), "{}", failure.message);
    assert_eq!(failure.log_tail, Some(vec!["core: bind failed".to_string()]));
    assert_eq!(world.leftovers(), 0);
}

#[test]
fn an_installer_with_no_answer_or_past_its_time_fails_with_its_last_lines() {
    let world = World::new();
    let registry = world.registry(&world.installer(json!({"stderr":"thread main panicked","stdout":"","exit":101})));
    let failure = prepare(&Job::new("j"), &registry, &world.data, &world.options()).unwrap_err();
    assert_eq!((failure.key.as_str(), failure.step), ("install.failed", Some(Step::ServiceStart)));
    assert_eq!(failure.log_tail, Some(vec!["thread main panicked".to_string()]));

    let world = World::new();
    // Before it writes install.json: still staging.
    let hangs = json!({"stderr":"waiting for the lock","delay_ms":30000});
    let registry = world.registry(&hangs);
    let quick = Options { installer_time: Duration::from_millis(500), ..world.options() };
    let started = Instant::now();
    let failure = prepare(&Job::new("j"), &registry, &world.data, &quick).unwrap_err();
    assert_eq!((failure.key.as_str(), failure.step), ("cli.timeout", Some(Step::Staging)));
    assert_eq!(failure.log_tail, Some(vec!["waiting for the lock".to_string()]));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(world.leftovers(), 0);
}

#[test]
fn a_cancel_during_the_download_stops_it_and_leaves_nothing() {
    let world = World::new();
    let registry = world.registry(&world.installer(json!({"response":{"ok":true}})));
    registry.slow(&registry.tarball_url(), Duration::from_millis(20));
    let job = std::sync::Arc::new(Job::new("j"));
    let canceller = {
        let job = job.clone();
        std::thread::spawn(move || {
            while job.progress().done.is_none() {
                std::thread::sleep(Duration::from_millis(5));
            }
            job.cancel()
        })
    };
    let failure = prepare(&job, &registry, &world.data, &world.options()).unwrap_err();
    assert!(canceller.join().unwrap(), "accepted while downloading");
    assert_eq!((failure.key.as_str(), failure.step), ("install.cancelled", Some(Step::Download)));
    assert_eq!(world.ran(), None);
    assert_eq!(world.leftovers(), 0);
}

#[test]
fn download_bounds_hold() {
    let world = World::new();
    let registry = world.registry(&world.installer(json!({"response":{"ok":true}})));
    let small = Options { download_limit: 100, ..world.options() };
    let failure = prepare(&Job::new("j"), &registry, &world.data, &small).unwrap_err();
    assert_eq!(failure.params, Some(json!({"check": "download-size"})));
    // A body shorter than it said.
    let bytes = registry.tarball_bytes();
    registry.serve_with_length(&registry.tarball_url(), bytes[..bytes.len() / 2].to_vec(), bytes.len() as u64);
    let failure = prepare(&Job::new("j"), &registry, &world.data, &world.options()).unwrap_err();
    assert_eq!((failure.key.as_str(), failure.step), ("install.network", Some(Step::Download)));
    registry.serve(&registry.tarball_url(), 503, vec![]);
    assert_eq!(prepare(&Job::new("j"), &registry, &world.data, &world.options()).unwrap_err().key, "install.network");
    assert_eq!(world.leftovers(), 0);
}

#[test]
fn the_private_directory_is_private_and_below_a_safe_root_only() {
    let world = World::new();
    let scratch = Scratch::create(&world.temp).unwrap();
    let meta = std::fs::metadata(scratch.path()).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    let path = scratch.path().to_path_buf();
    drop(scratch);
    assert!(!path.exists());
    std::fs::set_permissions(&world.temp, std::fs::Permissions::from_mode(0o777)).unwrap();
    let failure = Scratch::create(&world.temp.join(".")).unwrap_err();
    assert_eq!(failure.key, "install.unsafe", "a temporary directory others can write in, not sticky");
    std::fs::set_permissions(&world.temp, std::fs::Permissions::from_mode(0o1777)).unwrap();
    assert!(Scratch::create(&world.temp).is_ok(), "sticky, as /tmp");
    assert_eq!(Scratch::create(Path::new("relative")).unwrap_err().key, "install.unsafe");
}

#[test]
fn installer_keys_map_to_the_pages() {
    let failed = ExitStatus::from_raw(1 << 8);
    let answer = |key: &str| json!({"ok": false, "error": {"key": key, "params": {"detail": "d"}, "message": "m"}});
    for (theirs, ours, step) in [
        ("install.rolled-back", "install.rollback", Step::ServiceStart),
        ("install.rollback-failed", "install.rollback-failed", Step::ServiceStart),
        ("install.verify-failed", "service.start.failed", Step::ServiceStart),
        ("core.package-missing", "install.no-bundle", Step::Staging),
        ("core.package-mismatch", "install.authenticity", Step::Staging),
        ("service.busy", BUSY, Step::Staging),
        ("connector.failed", "install.failed", Step::Staging),
        ("install.failed", "install.failed", Step::Staging),
        ("something.new", "something.new", Step::Staging),
    ] {
        let failure = installer_failure(Some(&answer(theirs)), failed, Step::Staging);
        assert_eq!((failure.key.as_str(), failure.step), (ours, Some(step)), "{theirs}");
        assert_eq!(failure.message, "m");
        assert_eq!(failure.params.as_ref().unwrap()["detail"], "d");
    }
    let mismatch = installer_failure(Some(&answer("core.package-mismatch")), failed, Step::Staging);
    assert_eq!(mismatch.params, Some(json!({"detail": "d", "check": "manifest"})));
    let none = installer_failure(None, failed, Step::ServiceStart);
    assert_eq!((none.key.as_str(), none.params), ("install.failed", None));
    let shapeless = installer_failure(Some(&json!({"ok": false, "error": "boom"})), failed, Step::Staging);
    assert_eq!(shapeless.key, "install.failed");
}
