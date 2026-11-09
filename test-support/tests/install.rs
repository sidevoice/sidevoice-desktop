use serde_json::json;
use sidevoice_desktop_test_support::fake_registry::FakeRegistry;
use sidevoice_desktop_test_support::fixture;
use sidevoice_desktop_test_support::platform::storage::{StorageFixture, StorageFixtures};
use sidevoice_desktop_test_support::platform::temp;
use sidevoice_local_host::install::*;
use sidevoice_platform::fs::{Files, Fs, TestFiles};
use std::{
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
        let tmp = tempfile::Builder::new().prefix("svin").tempdir_in(temp::short_root()).unwrap();
        Files::shut_others_out(tmp.path());
        let data = tmp.path().join("d");
        let temp = tmp.path().join("t");
        std::fs::create_dir(&temp).unwrap();
        World { tmp, data, temp }
    }

    fn options(&self) -> Options {
        Options {
            package: Some(PLATFORM.into()),
            staging: StagingRoot { root: Root::Temp, path: self.temp.clone() },
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
    // Until core gap 1 (briefs CORE-GAPS-local-host.md): `sidevoice_platform::process::Child::output` keeps nothing of
    // a run it killed at its deadline, so a timeout has no last lines. Then: `Some(vec!["waiting for the lock"])`.
    assert_eq!(failure.log_tail, None);
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

const DAY: Duration = Duration::from_secs(24 * 3600);

fn in_temp(path: &Path) -> StagingRoot {
    StagingRoot { root: Root::Temp, path: path.to_path_buf() }
}

#[test]
fn the_private_directory_is_private_and_below_a_safe_root_only() {
    let world = World::new();
    let scratch = Scratch::create(&in_temp(&world.temp), DAY).unwrap();
    StorageFixtures::assert_private(scratch.path());
    let path = scratch.path().to_path_buf();
    drop(scratch);
    assert!(!path.exists());
    Files::let_others_change(&world.temp);
    let failure = Scratch::create(&in_temp(&world.temp.join(".")), DAY).unwrap_err();
    assert_eq!(failure.key, "install.unsafe", "a temporary directory others can write in, not sticky");
    assert_eq!(failure.step, Some(Step::Download));
    let params = failure.params.clone().unwrap();
    assert_eq!((&params["where"], &params["root"]), (&json!("scratch"), &json!("temp")), "{params}");
    // Windows names who may replace what is in it; Unix says owner or permissions, without saying which.
    assert!(["others-can-replace", "unsafe-ancestor"].contains(&params["reason"].as_str().unwrap()), "{params}");
    let report = failure.log_report();
    assert!(report.contains("where=scratch") && report.contains("root=temp"), "{report}");
    assert!(!report.contains(&*world.temp.to_string_lossy()), "no path in the log: {report}");
    if temp::share_sticky(&world.temp) {
        assert!(Scratch::create(&in_temp(&world.temp), DAY).is_ok(), "sticky, as /tmp");
    }
    let relative = Scratch::create(&in_temp(Path::new("relative")), DAY).unwrap_err();
    assert_eq!(relative.key, "install.unsafe");
    assert_eq!(relative.params, Some(json!({"where": "staging-root", "reason": "not-absolute", "root": "temp"})));
}

/// The report this fixes (sidevoice-desktop 0.4.0 on Windows: `install.unsafe` at `download`): a temporary directory
/// moved below a drive root, as `C:\Temp` inherits `C:\`'s "Authenticated Users: Modify", is refused and says so, and
/// the app's own folder, where Windows stages instead, is not affected by it. Only where an ordinary user can make a
/// folder below the volume's root (Windows; `/` is root's elsewhere).
#[test]
fn a_temporary_directory_below_a_drive_root_is_refused_with_why_and_the_app_folder_still_works() {
    let Some(below) = temp::below_volume_root() else { return };
    let failure = Scratch::create(&in_temp(below.path()), DAY).unwrap_err();
    assert_eq!((failure.key.as_str(), failure.step), ("install.unsafe", Some(Step::Download)));
    let params = failure.params.clone().unwrap();
    assert_eq!(params["where"], "scratch", "{params}");
    assert_eq!(params["reason"], "others-can-replace", "{params}");
    assert!(failure.log_report().contains("reason=others-can-replace"), "{}", failure.log_report());
    assert_eq!(std::fs::read_dir(below.path()).unwrap().count(), 0, "nothing left below the drive root");

    let world = World::new();
    let staging =
        staging_root(true, below.path().to_path_buf(), Some(&world.tmp.path().join("Sidevoice").join("Data")));
    assert_eq!(staging.root, Root::AppFolder);
    let scratch = Scratch::create(&staging, DAY).unwrap();
    StorageFixtures::assert_private(scratch.path());
}

fn app_folder(world: &World) -> StagingRoot {
    let standard_data = world.tmp.path().join("app").join("Data");
    staging_root(true, world.temp.clone(), Some(&standard_data))
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn the_app_folder_is_made_private_and_what_an_interrupted_install_left_there_is_removed() {
    let world = World::new();
    let staging = app_folder(&world);
    assert_eq!(staging.path, world.tmp.path().join("app").join(STAGING_DIR));
    assert!(!staging.path.exists());
    let scratch = Scratch::create(&staging, DAY).unwrap();
    StorageFixtures::assert_private(&staging.path);
    StorageFixtures::assert_private(scratch.path());
    drop(scratch);
    assert!(entries(&staging.path).is_empty(), "its private directory is gone");

    // What a killed install leaves: its private directory with the download in it. And what is not an install's.
    let left = staging.path.join("sidevoice-install-AbC123");
    Files::create_private_dir(&left).unwrap();
    std::fs::write(left.join("package.tgz"), b"partial").unwrap();
    Files::create_private_dir(&staging.path.join("other")).unwrap();
    std::fs::write(staging.path.join("sidevoice-install-file"), b"").unwrap();

    // Recent: maybe another install's, kept.
    drop(Scratch::create(&staging, DAY).unwrap());
    assert_eq!(entries(&staging.path), ["other", "sidevoice-install-AbC123", "sidevoice-install-file"]);
    // Older than the bound: removed, and only it.
    drop(Scratch::create(&staging, Duration::ZERO).unwrap());
    assert_eq!(entries(&staging.path), ["other", "sidevoice-install-file"]);
}

#[test]
fn an_app_folder_that_is_there_and_not_private_is_refused_not_repaired() {
    let world = World::new();
    let staging = app_folder(&world);
    std::fs::create_dir_all(&staging.path).unwrap();
    StorageFixtures::private(&staging.path);
    assert!(Scratch::create(&staging, DAY).is_ok(), "private, it is used as it is");
    StorageFixtures::readable(&staging.path);
    let failure = Scratch::create(&staging, DAY).unwrap_err();
    assert_eq!((failure.key.as_str(), failure.step), ("install.unsafe", Some(Step::Download)));
    assert_eq!(
        failure.params,
        Some(json!({"where": "staging-root", "reason": "open-to-others", "root": "app-folder"})),
        "{}",
        failure.message
    );
    assert_eq!(
        failure.log_report(),
        "install.unsafe at step download (reason=open-to-others, root=app-folder, where=staging-root)"
    );
    assert_eq!(StorageFixtures::others(&staging.path), sidevoice_platform::fs::Others::Read, "never repaired");
}

#[test]
fn an_install_staged_in_the_app_folder_runs_and_leaves_nothing_there() {
    let world = World::new();
    let registry =
        world.registry(&world.installer(json!({"response":{"ok":true,"action":"installed","version":"0.8.1"}})));
    let options = Options { staging: app_folder(&world), ..world.options() };
    let answer = prepare(&Job::new("j"), &registry, &world.data, &options).unwrap();
    assert_eq!(answer["action"], "installed");
    assert!(world.ran().is_some());
    assert!(entries(&options.staging.path).is_empty());
    assert_eq!(world.leftovers(), 0, "nothing in the temporary directory either");
}

#[test]
fn installer_keys_map_to_the_pages() {
    let world = World::new();
    let program = world.tmp.path().join(fixture::program_name());
    fixture::install(Path::new(env!("CARGO_BIN_EXE_fake-connector")), &program, &json!({"exit": 1})).unwrap();
    let failed = std::process::Command::new(&program).status().unwrap();
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

#[test]
fn what_the_installer_answers_reaches_the_log_only_as_structured_fields() {
    let world = World::new();
    let program = world.tmp.path().join(fixture::program_name());
    fixture::install(Path::new(env!("CARGO_BIN_EXE_fake-connector")), &program, &json!({"exit": 1})).unwrap();
    let failed = std::process::Command::new(&program).status().unwrap();
    let answer = json!({"ok": false, "error": {
        "key": "connector.failed",
        "message": "Authentication failed for https://u:pw-secret@example.invalid/x?code=sensitive-code",
        "params": {
            "detail": "unsafe private directory /tmp/sensitive",
            "token": "sensitive-value",
            "nested": {"password": "sensitive-nested"},
            "check": "manifest",
            "version": "1.2.3",
        },
    }});
    let mut failure = installer_failure(Some(&answer), failed, Step::Staging);
    failure.log_tail = Some(vec!["fetch https://example.invalid/d?X-Amz-Signature=sensitive-sig".into()]);
    let report = failure.log_report();
    assert!(
        !report.contains("sensitive") && !report.contains("pw-secret") && !report.contains("example.invalid"),
        "{report}"
    );
    assert!(report.starts_with("install.failed at step staging"), "{report}");
    assert!(report.contains("check=manifest") && report.contains("version=1.2.3"), "{report}");
    // The page still gets the failure whole.
    assert!(json!(failure).to_string().contains("sensitive-value"));
}
