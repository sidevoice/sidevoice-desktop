//! The app's side of the engine against a real `Engine` over a test host: the machine's own storage and capabilities
//! (`NativeHost`, in a directory of the test's), and downloads served from memory instead of the network. No model is
//! ever loaded here: what runs a model is the engine's, and CI's round trip on Apple Silicon runs it.

use super::*;
use sidevoice_engine::{async_trait, CatalogFragment, Download, Family, Fetcher, Storage};
use std::path::Path;

/// What one served file is: its bytes, and whether its download stalls after the first part until it is dropped.
struct Served {
    url: String,
    bytes: Vec<u8>,
    stalls: bool,
}

struct ServedFetcher(Vec<Served>);

struct Body {
    parts: Vec<Vec<u8>>,
    size: u64,
    stalls: bool,
}

#[async_trait]
impl Fetcher for ServedFetcher {
    async fn fetch(&self, url: &str) -> sidevoice_engine::Result<Box<dyn Download>> {
        let served = self.0.iter().find(|s| s.url == url).ok_or(sidevoice_engine::Error::new("download-failed"))?;
        let mut parts: Vec<Vec<u8>> = served.bytes.chunks(PART).map(<[u8]>::to_vec).collect();
        parts.reverse();
        Ok(Box::new(Body { parts, size: served.bytes.len() as u64, stalls: served.stalls }))
    }
}

#[async_trait]
impl Download for Body {
    fn size(&self) -> Option<u64> {
        Some(self.size)
    }

    async fn chunk(&mut self) -> sidevoice_engine::Result<Option<Vec<u8>>> {
        if self.stalls && (self.parts.len() as u64) * (PART as u64) < self.size {
            // The network stopped answering after the first part: only a cancel ends this.
            std::future::pending::<()>().await;
        }
        Ok(self.parts.pop())
    }
}

const PART: usize = 1000;

struct TestHost {
    native: NativeHost,
    fetcher: ServedFetcher,
}

impl Host for TestHost {
    fn capabilities(&self) -> sidevoice_engine::Capabilities {
        self.native.capabilities()
    }
    fn storage(&self) -> &dyn Storage {
        self.native.storage()
    }
    fn fetcher(&self) -> &dyn Fetcher {
        &self.fetcher
    }
}

/// A catalogue of one Whisper model, `whisper-test`, whose sherpa-onnx build is three files served by the test host,
/// and one Kokoro-like voice model whose build needs more memory than any machine has.
struct TestCatalog(Family, Family);

impl CatalogSource for TestCatalog {
    fn load(&self) -> sidevoice_engine::Result<CatalogFragment> {
        Ok(CatalogFragment { families: vec![self.0.clone(), self.1.clone()] })
    }
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn file(key: &str, served: &Served) -> serde_json::Value {
    serde_json::json!({ "key": key, "url": served.url, "sha256": sha256(&served.bytes), "bytes": served.bytes.len() })
}

fn served(name: &str, size: usize, stalls: bool) -> Served {
    Served { url: format!("https://test.invalid/{name}"), bytes: (0..size).map(|i| (i % 251) as u8).collect(), stalls }
}

struct Test {
    engines: NativeEngines,
    root: PathBuf,
    _runtime: tokio::runtime::Runtime,
}

impl Drop for Test {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The engines over a test host in a fresh directory; `stalls`: the encoder's download stops answering mid-way.
fn engines(name: &str, stalls: bool) -> Test {
    let root = std::env::temp_dir().join(format!("sidevoice-desktop-engine-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let files = vec![
        served("encoder.onnx", 5000, stalls),
        served("decoder.onnx", 3000, false),
        served("tokens.txt", 200, false),
    ];
    let whisper = serde_json::json!({
        "id": "whisper", "architecture": "whisper", "source": "https://test.invalid",
        "models": [{
            "id": "whisper-test", "capabilities": ["stt"], "parameters_m": 1, "languages": ["es", "en"], "license": "MIT",
            "builds": [{
                "id": "whisper-test/sherpa-onnx-int8", "backend": "sherpa-onnx", "precision": "int8",
                "memory": { "mb": 1, "source": "estimated", "basis": "test" },
                "files": [file("whisper.encoder", &files[0]), file("whisper.decoder", &files[1]), file("tokens", &files[2])]
            }]
        }]
    });
    let kokoro = serde_json::json!({
        "id": "kokoro", "architecture": "kokoro", "source": "https://test.invalid",
        "models": [{
            "id": "kokoro-test", "capabilities": ["tts"], "parameters_m": 1, "languages": ["es"], "license": "MIT",
            "voices": [{ "id": "ef_dora", "languages": ["es"] }],
            "builds": [{
                "id": "kokoro-test/sherpa-onnx-int8", "backend": "sherpa-onnx", "precision": "int8",
                "requires": { "accelerators": ["cpu"] },
                "memory": { "mb": 4000000, "source": "estimated", "basis": "test" },
                "files": [file("kokoro.model", &files[2])]
            }]
        }]
    });
    let catalog = TestCatalog(serde_json::from_value(whisper).unwrap(), serde_json::from_value(kokoro).unwrap());
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let host = TestHost { native: NativeHost::new(&root).unwrap(), fetcher: ServedFetcher(files) };
    let engines = NativeEngines::with(Box::new(host), vec![Box::new(catalog)], runtime.handle().clone()).unwrap();
    Test { engines, root, _runtime: runtime }
}

/// Files the engine's storage holds being written: none once an install has ended, however it ended.
fn partial(root: &Path) -> Vec<String> {
    let dir = root.join("partial");
    std::fs::read_dir(dir)
        .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default()
}

#[test]
fn capabilities_are_this_device_what_the_engine_runs_here_and_its_memory() {
    let test = engines("capabilities", false);
    let json = serde_json::to_value(&test.engines.device).unwrap();
    assert_eq!(json["runs"], "native");
    assert_eq!(json["os"], std::env::consts::OS);
    assert_eq!(json["arch"], std::env::consts::ARCH);
    assert_eq!(json["has"], serde_json::json!(["cpu"]), "sherpa-onnx runs on the CPU in this build of the engine");
    assert!(json["memory_mb"].as_u64().is_some());
}

#[test]
fn what_does_not_run_here_is_refused_by_key_before_anything_is_downloaded_or_loaded() {
    let test = engines("refusals", false);
    let engines = &test.engines;
    let key = |r: Result<Load, Error>| r.unwrap_err().key;
    assert_eq!(key(engines.load("whisper-test", "transformers-js", None)), "engine_unsupported");
    // whisper.cpp is in this build of the engine; the test catalogue has no build of this model for it.
    assert_eq!(key(engines.load("whisper-test", "whisper-cpp", None)), "build_missing");
    assert_eq!(key(engines.load("whisper-nope", "sherpa-onnx", None)), "model_unknown");
    assert_eq!(key(engines.load("whisper-test", "sherpa-onnx", None)), "not_installed");
    let unusable = engines.load("whisper-test", "sherpa-onnx", Some("metal")).unwrap_err();
    assert_eq!(
        serde_json::to_value(&unusable).unwrap(),
        serde_json::json!({
            "key": "accelerator_unusable", "model": "whisper-test", "engine": "sherpa-onnx", "accelerator": "metal",
            "usable": ["cpu"], "message": "whisper-test on sherpa-onnx cannot use metal here; it can use cpu."
        })
    );
    let memory = engines.load("kokoro-test", "sherpa-onnx", None).unwrap_err();
    assert_eq!((memory.key, &memory.params["needed_mb"]), ("model_needs_memory", &serde_json::json!(4000000)));
    let wrong = engines.synthesize("whisper-test", "sherpa-onnx", None, "ef_dora", 1.0, "hola").unwrap_err();
    assert_eq!(wrong.key, "model_wrong_task");
    let wrong = engines.transcribe("kokoro-test", "sherpa-onnx", None, "es", &[], 16_000).unwrap_err();
    assert_eq!(wrong.key, "model_needs_memory", "what does not run here is refused first");
    let mut calls = 0;
    assert_eq!(
        engines.install("kokoro-test", "sherpa-onnx", &mut |_, _| calls += 1).unwrap_err().key,
        "model_needs_memory"
    );
    assert_eq!(calls, 0, "refused before it starts");
    assert!(engines.installed().unwrap().is_empty());
    assert!(engines.loaded().is_empty());
}

#[test]
fn an_install_reports_its_bytes_from_zero_to_the_total_and_lists_the_build_by_model_and_engine() {
    let test = engines("install", false);
    let engines = &test.engines;
    let mut reports = Vec::new();
    engines.install("whisper-test", "sherpa-onnx", &mut |done, total| reports.push((done, total))).unwrap();
    assert_eq!(reports.first(), Some(&(0, 8200)), "(0, total) first");
    assert_eq!(reports.last(), Some(&(8200, 8200)));
    assert!(reports.windows(2).all(|w| w[0].0 <= w[1].0), "never backwards: {reports:?}");
    assert!(reports.len() > 4, "as it goes: {reports:?}");
    assert_eq!(
        serde_json::to_value(engines.installed().unwrap()).unwrap(),
        serde_json::json!([{ "model": "whisper-test", "engine": "sherpa-onnx" }])
    );
    let disk = serde_json::to_value(engines.on_disk().unwrap()).unwrap();
    assert_eq!(
        disk,
        serde_json::json!({ "engines": [], "builds": [{
            "model": "whisper-test", "label": "whisper-test", "engine": "sherpa-onnx", "task": "stt", "bytes": 8200
        }]})
    );
    let mut again = Vec::new();
    engines.install("whisper-test", "sherpa-onnx", &mut |done, total| again.push((done, total))).unwrap();
    assert_eq!(again, [(0, 0)], "nothing left to download");
    assert!(partial(&test.root).is_empty());
}

#[test]
fn a_cancel_stops_a_download_the_network_stopped_answering_and_leaves_nothing() {
    let test = engines("cancel", true);
    let engines = &test.engines;
    let jobs = Jobs::default();
    std::thread::scope(|scope| {
        let install = scope.spawn(|| engines.install_job(&jobs, "j1", "whisper-test", "sherpa-onnx"));
        let progress = loop {
            match jobs.get("j1") {
                Some(p) if p.done > 0 => break p,
                _ => std::thread::sleep(Duration::from_millis(2)),
            }
        };
        assert_eq!(
            (progress.job.as_str(), progress.model.as_str(), progress.engine.as_str()),
            ("j1", "whisper-test", "sherpa-onnx")
        );
        assert!(progress.done < progress.total, "mid-download: {progress:?}");
        assert!(jobs.cancel("j1"));
        let cancelled = install.join().unwrap().unwrap_err();
        assert_eq!(
            serde_json::to_value(&cancelled).unwrap(),
            serde_json::json!({ "key": "install_cancelled", "message": "The download was cancelled." })
        );
    });
    assert_eq!(jobs.get("j1"), None);
    assert!(!jobs.cancel("j1"), "an ended job is not cancelled again");
    assert!(engines.installed().unwrap().is_empty());
    assert!(partial(&test.root).is_empty(), "nothing half downloaded is kept: {:?}", partial(&test.root));
    assert!(engines.installing.try_lock().is_ok(), "the install lock is free");
}

#[test]
fn a_job_waiting_for_another_reports_nothing_and_is_cancelled_where_it_waits() {
    let test = engines("waiting", false);
    let engines = &test.engines;
    let jobs = Jobs::default();
    let held = engines.installing.lock().unwrap(); // another install is running
    std::thread::scope(|scope| {
        let waiting = scope.spawn(|| engines.install_job(&jobs, "w", "whisper-test", "sherpa-onnx"));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(jobs.get("w"), None, "waiting: no progress");
        assert!(jobs.cancel("w"), "but known, and cancellable");
        assert_eq!(waiting.join().unwrap().unwrap_err().key, "install_cancelled", "without waiting for the other");
    });
    drop(held);
    assert!(jobs.cancel("early"), "not arrived yet: it will be refused");
    assert_eq!(
        engines.install_job(&jobs, "early", "whisper-test", "sherpa-onnx").unwrap_err().key,
        "install_cancelled"
    );
    engines.install_job(&jobs, "later", "whisper-test", "sherpa-onnx").unwrap();
    assert_eq!(engines.installed().unwrap().len(), 1);
}

#[test]
fn a_load_the_ci_probe_refuses_is_runtime_failed_and_leaves_memory_as_it_was() {
    let mut test = engines("refused", false);
    test.engines.faults.refuse_load = vec!["whisper-test".into()];
    let engines = &test.engines;
    engines.install("whisper-test", "sherpa-onnx", &mut |_, _| {}).unwrap();
    let refused = engines.load("whisper-test", "sherpa-onnx", Some("cpu")).unwrap_err();
    assert_eq!(refused.key, "runtime_failed");
    assert!(engines.loaded().is_empty());
    // The page unloaded it while it loaded: a cancel, not a failure (N03).
    engines.unload("whisper-test", "sherpa-onnx", None);
    assert!(engines.loaded().is_empty());
}

#[test]
fn the_call_state_is_the_room_s_and_a_call_starting_is_the_moment_to_preload() {
    let test = engines("calls", false);
    let engines = &test.engines;
    let now = Instant::now();
    assert!(engines.call_changed(true, now));
    assert!(!engines.call_changed(true, now), "already on");
    assert!(!engines.call_changed(false, now));
    assert!(engines.preload().is_empty(), "nothing was unloaded idle");
    assert!(engines.unload_idle(now + IDLE_UNLOAD).is_empty());
}

#[test]
fn a_late_cancel_removes_only_a_build_that_is_all_of_the_model_on_disk() {
    assert_eq!(undo(Downloaded { this: false, others: false }), Undo::Nothing);
    assert_eq!(undo(Downloaded { this: false, others: true }), Undo::Nothing);
    assert_eq!(undo(Downloaded { this: true, others: false }), Undo::Remove);
    assert_eq!(undo(Downloaded { this: true, others: true }), Undo::Keep, "the other build would go with it");
}

#[test]
fn rolling_back_a_late_cancel_removes_the_build_or_keeps_it_and_says_so() {
    let test = engines("roll-back", false);
    let engines = &test.engines;
    engines.install("whisper-test", "sherpa-onnx", &mut |_, _| {}).unwrap();
    let build = "whisper-test/sherpa-onnx-int8";
    assert!(!engines.others_installed("whisper-test", build).unwrap(), "its own build is not another");
    assert!(engines.others_installed("whisper-test", "whisper-test/another").unwrap());

    let held = engines.install_lock(&|| false).unwrap();
    // Another build was installed before this job: the engine removes whole models, so this one stays.
    let kept = engines.roll_back(&held, "whisper-test", "sherpa-onnx", Downloaded { this: true, others: true });
    assert_eq!(kept.key, "install_cancel_late");
    assert_eq!(engines.installed().unwrap().len(), 1, "nothing was removed");

    // The build is all of the model on disk: it goes, and the job is a plain cancel.
    let removed = engines.roll_back(&held, "whisper-test", "sherpa-onnx", Downloaded { this: true, others: false });
    assert_eq!(removed.key, "install_cancelled");
    assert!(engines.installed().unwrap().is_empty());
}

/// A job holds the install lock until it has ended, its roll-back included (`roll_back` takes the lock): another
/// install asking meanwhile waits, so it cannot put another build of the model on disk before a late cancel removes the
/// model.
#[test]
fn another_install_waits_for_the_lock_a_job_holds_to_roll_back() {
    let test = engines("roll-back-lock", false);
    let engines = &test.engines;
    let held = engines.install_lock(&|| false).unwrap();
    let asked = std::time::Instant::now();
    let waited = engines.install_lock(&|| asked.elapsed() > Duration::from_millis(200));
    assert_eq!(waited.unwrap_err().key, "install_cancelled", "it waited until told to stop");
    drop(held);
    assert!(engines.install_lock(&|| false).is_ok());
}
