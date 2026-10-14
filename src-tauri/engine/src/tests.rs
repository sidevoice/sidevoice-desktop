//! The app's side of the engine against a real `Engine` over a test host: the machine's own storage and capabilities
//! (`NativeHost`, in a directory of the test's), and downloads served from memory instead of the network. No model is
//! ever loaded here: what runs a model is the engine's, through the voice call.

use super::*;
use sidevoice_engine::{
    async_trait, CatalogFragment, Credentials, Download, Family, Fetcher, HttpClient, HttpRequest, HttpResponse,
    Storage,
};
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
    fn http(&self) -> &dyn HttpClient {
        self.native.http()
    }
    fn credentials(&self) -> &dyn Credentials {
        self.native.credentials()
    }
}

/// A catalogue of one Whisper model, `whisper-test`, whose sherpa-onnx build is three files served by the test host,
/// and one Kokoro-like voice model whose build needs more memory than any machine has (and, `shared`, more: see
/// `engines_with`).
struct TestCatalog(Vec<Family>);

impl CatalogSource for TestCatalog {
    fn load(&self) -> sidevoice_engine::Result<CatalogFragment> {
        Ok(CatalogFragment { families: self.0.clone() })
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
    engines_with(name, stalls, false)
}

/// `engines`; `shared`: another model, `echo-test`, whose one build's files are the same bytes as `whisper-test`'s int8
/// build's (the engine stores a file once, by its SHA-256, and links it to each build).
fn engines_with(name: &str, stalls: bool, shared: bool) -> Test {
    let root = std::env::temp_dir().join(format!("sidevoice-desktop-engine-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let files = vec![
        served("encoder.onnx", 5000, stalls),
        served("decoder.onnx", 3000, false),
        served("tokens.txt", 200, false),
        served("whisper-fp32.onnx", 300, false),
    ];
    let whisper = serde_json::json!({
        "id": "whisper", "architecture": "whisper", "source": "https://test.invalid",
        "models": [{
            "id": "whisper-test", "capabilities": ["stt"], "parameters_m": 1, "languages": ["es", "en"], "license": "MIT",
            "builds": [{
                "id": "whisper-test/sherpa-onnx-int8", "backend": "sherpa-onnx", "precision": "int8",
                "memory": { "mb": 1, "source": "estimated", "basis": "test" },
                "files": [file("whisper.encoder", &files[0]), file("whisper.decoder", &files[1]), file("tokens", &files[2])]
            }, {
                "id": "whisper-test/sherpa-onnx-fp32", "backend": "sherpa-onnx", "precision": "fp32",
                "memory": { "mb": 1, "source": "estimated", "basis": "test" },
                "files": [file("whisper.model", &files[3])]
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
    let mut families = vec![whisper, kokoro];
    if shared {
        let mut echo = families[0].clone();
        echo["id"] = "echo".into();
        echo["models"][0]["id"] = "echo-test".into();
        echo["models"][0]["builds"] = serde_json::json!([families[0]["models"][0]["builds"][0].clone()]);
        echo["models"][0]["builds"][0]["id"] = "echo-test/sherpa-onnx-int8".into();
        families.push(echo);
    }
    let catalog = TestCatalog(families.into_iter().map(|f| serde_json::from_value(f).unwrap()).collect());
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let host = TestHost { native: NativeHost::new(&root).unwrap(), fetcher: ServedFetcher(files) };
    let engines = NativeEngines::with(Box::new(host), vec![Box::new(catalog)], runtime.handle().clone()).unwrap();
    Test { engines, root, _runtime: runtime }
}

/// The build the page asks for by engine, as the tests mostly install it.
const ENGINE: Wanted<'static> = Wanted { build: None, engine: Some("sherpa-onnx") };

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
fn what_does_not_run_here_is_refused_by_key_before_anything_is_downloaded() {
    let test = engines("refusals", false);
    let engines = &test.engines;
    let key = |model: &str, engine: &str| {
        engines.install(model, Wanted { build: None, engine: Some(engine) }, &mut |_, _| {}).unwrap_err().key
    };
    assert_eq!(key("whisper-test", "transformers-js"), "engine_unsupported");
    // whisper.cpp is in this build of the engine; the test catalogue has no build of this model for it.
    assert_eq!(key("whisper-test", "whisper-cpp"), "build_missing");
    assert_eq!(key("whisper-nope", "sherpa-onnx"), "model_unknown");
    let mut calls = 0;
    let memory = engines.install("kokoro-test", ENGINE, &mut |_, _| calls += 1).unwrap_err();
    assert_eq!((memory.key, &memory.params["needed_mb"]), ("model_needs_memory", &serde_json::json!(4000000)));
    assert_eq!(calls, 0, "refused before it starts");
    assert!(engines.installed().unwrap().is_empty());
}

/// The page names a build by its catalogue id (the web's `install(model, {build, engine})`), or nothing: the build
/// named, never another, and with neither the one the catalogue recommends here.
#[test]
fn an_install_takes_the_build_named_or_else_the_recommended_one() {
    let test = engines("build", false);
    let engines = &test.engines;
    let key = |model: &str, build: Option<&str>, engine: Option<&str>| {
        engines.install(model, Wanted { build, engine }, &mut |_, _| {}).unwrap_err().key
    };
    assert_eq!(key("whisper-test", Some("whisper-test/whisper-cpp"), None), "build_missing", "an unknown build");
    assert_eq!(
        key("whisper-test", Some("whisper-test/sherpa-onnx-int8"), Some("whisper-cpp")),
        "build_missing",
        "not that engine's"
    );
    assert_eq!(
        key("whisper-test", Some("whisper-test/sherpa-onnx-int8"), Some("transformers-js")),
        "engine_unsupported"
    );
    assert_eq!(key("whisper-nope", None, None), "model_unknown");
    assert_eq!(
        key("whisper-test", Some("kokoro-test/sherpa-onnx-int8"), None),
        "build_missing",
        "another model's build"
    );
    assert_eq!(
        key("kokoro-test", Some("kokoro-test/sherpa-onnx-int8"), None),
        "model_needs_memory",
        "named, still checked"
    );
    // Nothing of kokoro-test runs here: with no build named, its own build says why.
    assert_eq!(key("kokoro-test", None, None), "model_needs_memory");
    assert!(engines.installed().unwrap().is_empty(), "refused before anything is downloaded");

    let named = Wanted { build: Some("whisper-test/sherpa-onnx-int8"), engine: Some("sherpa-onnx") };
    engines.install("whisper-test", named, &mut |_, _| {}).unwrap();
    assert_eq!(
        engines.installed().unwrap(),
        [Installed { model: "whisper-test".into(), engine: "sherpa-onnx".into() }]
    );
    let mut again = Vec::new();
    engines.install("whisper-test", Wanted::default(), &mut |done, total| again.push((done, total))).unwrap();
    assert_eq!(again, [(0, 0)], "the recommended build is the one on disk: nothing to download");

    let jobs = Jobs::default();
    let build_only = Wanted { build: Some("whisper-test/sherpa-onnx-int8"), engine: None };
    engines.install_job(&jobs, "by-build", "whisper-test", build_only).unwrap();
    let nope = Wanted { build: Some("nope"), engine: None };
    assert_eq!(engines.install_job(&jobs, "refused", "whisper-test", nope).unwrap_err().key, "build_missing");
    assert!(!jobs.cancel("refused"), "a refused job has ended: nothing to cancel");
    assert!(jobs.cancel("early"), "cancelled before it arrives");
    let early = engines.install_job(&jobs, "early", "whisper-test", nope).unwrap_err();
    assert_eq!(early.key, "install_cancelled", "a cancel that came first wins over the refusal");
}

/// A build that is not the one an install by engine would choose, installed by its id, is listed as on disk.
#[test]
fn a_build_installed_by_its_id_is_listed_on_disk() {
    let test = engines("other-build", false);
    let engines = &test.engines;
    let fp32 = Wanted { build: Some("whisper-test/sherpa-onnx-fp32"), engine: None };
    let mut reports = Vec::new();
    engines.install("whisper-test", fp32, &mut |done, total| reports.push((done, total))).unwrap();
    assert_eq!(reports.last(), Some(&(300, 300)), "that build's bytes, not the int8 one's");
    assert_eq!(
        engines.installed().unwrap(),
        [Installed { model: "whisper-test".into(), engine: "sherpa-onnx".into() }]
    );
    let disk = serde_json::to_value(engines.on_disk().unwrap()).unwrap();
    assert_eq!(disk["builds"][0]["bytes"], 300);
    assert_eq!(disk["builds"].as_array().unwrap().len(), 1);
}

#[test]
fn an_install_reports_its_bytes_from_zero_to_the_total_and_lists_the_build_by_model_and_engine() {
    let test = engines("install", false);
    let engines = &test.engines;
    let mut reports = Vec::new();
    engines.install("whisper-test", ENGINE, &mut |done, total| reports.push((done, total))).unwrap();
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
    engines.install("whisper-test", ENGINE, &mut |done, total| again.push((done, total))).unwrap();
    assert_eq!(again, [(0, 0)], "nothing left to download");
    assert!(partial(&test.root).is_empty());
}

#[test]
fn a_cancel_stops_a_download_the_network_stopped_answering_and_leaves_nothing() {
    let test = engines("cancel", true);
    let engines = &test.engines;
    let jobs = Jobs::default();
    std::thread::scope(|scope| {
        let install = scope.spawn(|| engines.install_job(&jobs, "j1", "whisper-test", ENGINE));
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
        let waiting = scope.spawn(|| engines.install_job(&jobs, "w", "whisper-test", ENGINE));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(jobs.get("w"), None, "waiting: no progress");
        assert!(jobs.cancel("w"), "but known, and cancellable");
        assert_eq!(waiting.join().unwrap().unwrap_err().key, "install_cancelled", "without waiting for the other");
    });
    drop(held);
    assert!(jobs.cancel("early"), "not arrived yet: it will be refused");
    assert_eq!(engines.install_job(&jobs, "early", "whisper-test", ENGINE).unwrap_err().key, "install_cancelled");
    engines.install_job(&jobs, "later", "whisper-test", ENGINE).unwrap();
    assert_eq!(engines.installed().unwrap().len(), 1);
}

#[test]
fn the_engine_is_shared_with_the_voice_call() {
    let test = engines("shared", false);
    let engine = test.engines.engine();
    let models = test.engines.models().unwrap();
    assert_eq!(models.len(), 2);
    assert!(std::sync::Arc::ptr_eq(&engine, &test.engines.engine()));
}

#[test]
fn rolling_back_a_late_cancel_removes_the_build_it_installed() {
    let test = engines("roll-back", false);
    let engines = &test.engines;
    engines.install("whisper-test", ENGINE, &mut |_, _| {}).unwrap();
    let held = engines.install_lock(&|| false).unwrap();
    // Nothing was downloaded: nothing to remove.
    assert_eq!(engines.roll_back(&held, "whisper-test", "sherpa-onnx", Downloaded::Nothing).key, "install_cancelled");
    assert_eq!(engines.installed().unwrap().len(), 1);
    // The build it downloaded goes, and only it (the engine removes one build when it is named).
    let build = Downloaded::Build("whisper-test/sherpa-onnx-int8".into());
    assert_eq!(engines.roll_back(&held, "whisper-test", "sherpa-onnx", build).key, "install_cancelled");
    assert!(engines.installed().unwrap().is_empty());
}

/// The page's list of catalogues: the local one first, in the engine's JavaScript names, then each provider, which
/// without a key lists nothing and says why. Nothing goes to the network for that.
#[test]
fn the_catalogues_are_the_local_one_then_each_provider_in_the_pages_shape() {
    let test = engines("catalogs", false);
    let views = test.engines.catalogs().unwrap();
    assert_eq!(views[0].id, LOCAL_CATALOG);
    assert_eq!(views[0].name, None);
    let local = serde_json::to_value(&views[0]).unwrap();
    assert_eq!(local["status"], serde_json::json!({"stale": false}));
    let whisper = local["models"].as_array().unwrap().iter().find(|m| m["id"] == "whisper-test").unwrap();
    assert_eq!(whisper["capabilities"], serde_json::json!(["stt"]));
    for key in ["family", "parametersM", "license", "installed", "builds"] {
        assert!(whisper.get(key).is_some(), "{key} in {whisper}");
    }
    assert_eq!(whisper.get("lastUsedAt"), None, "not installed: the engine does not know");
    let build = &whisper["builds"][0];
    for key in ["id", "backend", "precision", "downloadBytes", "memoryMb", "available", "reasons", "installed"] {
        assert!(build.get(key).is_some(), "{key} in {build}");
    }
    assert_eq!(build["accelerator"], "cpu");
    assert_eq!(build["acceleratorClass"], "cpu");
    for remote in &views[1..] {
        assert!(remote.name.is_some(), "a provider has a name");
        assert!(remote.models.is_empty(), "{} lists nothing without a key", remote.id);
        assert_eq!(remote.status.reason.as_ref().map(|r| r.code.as_str()), Some("credential-missing"));
    }
    assert_eq!(test.engines.refresh("no-such").unwrap_err().key, "catalog_not_found");
}

/// The engine owns the classification; the bridge uses its stable ids and omits both fields when no accelerator is
/// available for a build. Serialize the actual local-model mapping, not a standalone BuildView fixture.
#[test]
fn a_local_build_exposes_the_engines_accelerator_class() {
    let test = engines("accelerator-class", false);
    let mut model = test.engines.models().unwrap().into_iter().find(|m| m.id == "whisper-test").unwrap();
    for (accelerator, id, class) in [
        (Some(Accelerator::Metal), Some("metal"), Some("gpu")),
        (Some(Accelerator::CoreMl), Some("unknown"), Some("neural-engine")),
        (Some(Accelerator::Cpu), Some("cpu"), Some("cpu")),
        (None, None, None),
    ] {
        model.builds[0].accelerator = accelerator;
        let json = serde_json::to_value(catalogs::local_model(&model)).unwrap();
        let build = &json["builds"][0];
        match class {
            Some(id) => assert_eq!(build["acceleratorClass"], id),
            None => assert!(build.get("acceleratorClass").is_none(), "{build}"),
        }
        match id {
            Some(id) => assert_eq!(build["accelerator"], id),
            None => assert!(build.get("accelerator").is_none(), "{build}"),
        }
        assert!(build.get("accelerator_class").is_none(), "{build}");
    }
}

/// The two Supertonic models have different stable voice ids. Keep every display name, gender and language through
/// the bundled engine catalogue and this bridge, without downloading any model.
#[test]
fn the_bundled_supertonic_voices_reach_the_page() {
    let root = std::env::temp_dir().join(format!("sidevoice-supertonic-catalog-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let engines = NativeEngines::new(&root, sidevoice_engine::NoCredentials, runtime.handle().clone()).unwrap();
    let models = serde_json::to_value(&engines.catalogs().unwrap()[0].models).unwrap();
    for (model_id, ids, languages) in [
        (
            "supertonic-2",
            (0..10).map(|i| if i < 5 { format!("F{}", i + 1) } else { format!("M{}", i - 4) }).collect::<Vec<_>>(),
            vec!["en", "ko", "es", "pt", "fr"],
        ),
        (
            "supertonic-3",
            (0..10).map(|i| i.to_string()).collect::<Vec<_>>(),
            vec![
                "en", "ko", "ja", "ar", "bg", "cs", "da", "de", "el", "es", "et", "fi", "fr", "hi", "hr", "hu", "id",
                "it", "lt", "lv", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "tr", "uk", "vi",
            ],
        ),
    ] {
        let model = models.as_array().unwrap().iter().find(|model| model["id"] == model_id).unwrap();
        let expected: Vec<_> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let name = if i < 5 { format!("F{}", i + 1) } else { format!("M{}", i - 4) };
                let gender = if i < 5 { "female" } else { "male" };
                serde_json::json!({"id": id, "name": name, "gender": gender, "languages": languages})
            })
            .collect();
        assert_eq!(model["voices"], serde_json::json!(expected), "{model_id}");
    }
    drop(engines);
    let _ = std::fs::remove_dir_all(root);
}

/// When the engine last loaded a local model reaches the page as `lastUsedAt`, the engine's milliseconds since the
/// Unix epoch as a number, and is absent while the engine does not know. (No model is loaded here: the time is set on
/// the engine's own listing of the model.)
#[test]
fn a_local_model_says_when_it_was_last_used_in_milliseconds_or_nothing() {
    let test = engines("last-used", false);
    let mut model = test.engines.models().unwrap().into_iter().find(|m| m.id == "whisper-test").unwrap();
    model.installed = true;
    model.last_used_at = Some(1_760_000_000_123);
    let json = serde_json::to_value(catalogs::local_model(&model)).unwrap();
    assert_eq!(json["lastUsedAt"], serde_json::json!(1_760_000_000_123_u64));
    assert!(json["lastUsedAt"].is_u64());
    model.last_used_at = None;
    let json = serde_json::to_value(catalogs::local_model(&model)).unwrap();
    assert_eq!(json.get("lastUsedAt"), None, "{json}");
    assert_eq!(json.get("last_used_at"), None, "{json}");
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

/// `uninstall` takes the model off the computer, every build of it; one that is not on disk is nothing to do, and a
/// model the catalogue does not list is refused by key, with the engine's code the page reads.
#[test]
fn an_uninstall_takes_every_build_of_the_model_off_the_disk() {
    let test = engines("uninstall", false);
    let engines = &test.engines;
    let jobs = Jobs::default();
    engines.install("whisper-test", ENGINE, &mut |_, _| {}).unwrap();
    assert_eq!(engines.installed().unwrap().len(), 1);
    engines.uninstall(&jobs, "whisper-test").unwrap();
    assert!(engines.installed().unwrap().is_empty());
    assert!(engines.on_disk().unwrap().builds.is_empty());
    let models = engines.models().unwrap();
    let whisper = models.iter().find(|m| m.id == "whisper-test").unwrap();
    assert!(whisper.builds.iter().all(|b| !b.installed), "no build of it is left");
    engines.uninstall(&jobs, "whisper-test").expect("not on disk: nothing to do");
    let unknown = serde_json::to_value(engines.uninstall(&jobs, "whisper-nope").unwrap_err()).unwrap();
    assert_eq!((unknown["key"].as_str(), unknown["code"].as_str()), (Some("model_unknown"), Some("model-not-found")));
    // And it installs again afterwards.
    engines.install("whisper-test", ENGINE, &mut |_, _| {}).unwrap();
    assert_eq!(engines.installed().unwrap().len(), 1);
}

/// Every build of the model goes, and a file another model's build links stays: that model is still whole on disk.
#[test]
fn an_uninstall_removes_every_build_and_keeps_the_files_another_model_shares() {
    let test = engines_with("uninstall-builds", false, true);
    let engines = &test.engines;
    let engine = engines.engine();
    let installed = |model: &str| -> Vec<(String, bool)> {
        let models = engines.models().unwrap();
        let model = models.iter().find(|m| m.id == model).unwrap();
        model.builds.iter().map(|b| (b.id.clone(), b.installed)).collect()
    };
    let install = |model: &str, build: &str| {
        let cancel = Cancel::new();
        test._runtime.block_on(engine.install(model, Some(build), &|_| {}, &cancel)).unwrap();
    };
    install("whisper-test", "whisper-test/sherpa-onnx-int8");
    install("whisper-test", "whisper-test/sherpa-onnx-fp32");
    install("echo-test", "echo-test/sherpa-onnx-int8");
    assert!(installed("whisper-test").iter().all(|(_, on)| *on));
    engines.uninstall(&Jobs::default(), "whisper-test").unwrap();
    assert_eq!(
        installed("whisper-test"),
        [("whisper-test/sherpa-onnx-int8".to_string(), false), ("whisper-test/sherpa-onnx-fp32".to_string(), false)],
        "both builds gone"
    );
    assert_eq!(installed("echo-test"), [("echo-test/sherpa-onnx-int8".to_string(), true)], "the other model stays");
    let folder = test.root.join("models").join("echo-test/sherpa-onnx-int8");
    let files: Vec<_> = std::fs::read_dir(&folder).map(|d| d.flatten().collect()).unwrap_or_default();
    assert_eq!(files.len(), 3, "its files are whole in {folder:?}");
    for file in files {
        assert!(std::fs::metadata(file.path()).unwrap().len() > 0);
    }
}

/// An install job of the model, waiting or running, refuses its uninstall until the page cancels it: the job would
/// otherwise put back what was removed, or lose it once it resolved. A job of another model does not.
#[test]
fn an_uninstall_is_refused_while_a_job_of_the_model_is_installing() {
    let test = engines("uninstall-installing", false);
    let engines = &test.engines;
    let jobs = Jobs::default();
    engines.install("whisper-test", ENGINE, &mut |_, _| {}).unwrap();
    assert!(jobs.begin("j", "whisper-test", "sherpa-onnx"));
    let refused = engines.uninstall(&jobs, "whisper-test").unwrap_err();
    assert_eq!((refused.key, &refused.params["code"]), ("model_installing", &serde_json::json!("model-installing")));
    assert_eq!(engines.installed().unwrap().len(), 1, "nothing removed");
    assert!(jobs.begin("k", "kokoro-test", "sherpa-onnx"));
    assert!(jobs.cancel("j"));
    engines.uninstall(&jobs, "whisper-test").expect("a cancelled job and another model's do not refuse it");
    assert!(engines.installed().unwrap().is_empty());
}

/// It waits for the install that holds the install lock, so no install puts a build of the model on disk while it
/// removes.
#[test]
fn an_uninstall_waits_for_the_install_lock() {
    let test = engines("uninstall-lock", false);
    let engines = &test.engines;
    let jobs = Jobs::default();
    engines.install("whisper-test", ENGINE, &mut |_, _| {}).unwrap();
    let held = engines.install_lock(&|| false).unwrap(); // another install is running
    std::thread::scope(|scope| {
        let removing = scope.spawn(|| engines.uninstall(&jobs, "whisper-test"));
        std::thread::sleep(Duration::from_millis(150));
        assert!(!removing.is_finished(), "it waits for the install ahead of it");
        drop(held);
        removing.join().unwrap().unwrap();
    });
    assert!(engines.installed().unwrap().is_empty());
    assert!(engines.installing.try_lock().is_ok(), "the install lock is free");
}

// ---- Provider keys: tried without being kept, and a provider asked again ----

const OPENAI_SPEC: &str = "https://raw.githubusercontent.com/openai/openai-openapi/main/openapi.json";
const OPENAI_MODELS: &str = "https://api.openai.com/v1/models";

/// A fake OpenAI: its spec, and its models for the key `good`; the key `scoped` is known but may not list (403), any
/// other key is refused (401). A request with the key `hold` waits until `released` before it is refused. Every request to it is recorded with the key it carried.
#[derive(Default)]
struct FakeOpenAi {
    asked: Mutex<Vec<(String, String)>>,
    holding: std::sync::atomic::AtomicBool,
    released: std::sync::atomic::AtomicBool,
}

impl FakeOpenAi {
    /// The keys the models listing was asked with, in order.
    fn listed_with(&self) -> Vec<String> {
        guard(&self.asked).iter().filter(|(url, _)| url == OPENAI_MODELS).map(|(_, key)| key.clone()).collect()
    }
}

/// The spec the engine reads OpenAI's facts from, cut to what it reads (sidevoice-engine's own test fixture).
fn openai_spec() -> serde_json::Value {
    serde_json::json!({ "components": { "schemas": {
        "CreateTranscriptionRequest": { "properties": {
            "model": { "anyOf": [{ "type": "string" }, { "type": "string", "enum": ["whisper-1", "gpt-4o-transcribe"] }] },
            "language": { "type": "string" },
        }},
        "CreateSpeechRequest": { "properties": {
            "model": { "anyOf": [{ "type": "string" }, { "type": "string", "enum": ["tts-1"] }] },
            "voice": { "anyOf": [{ "type": "string" }, { "type": "string", "enum": ["alloy", "nova"] }] },
            "speed": { "type": "number", "minimum": 0.25, "maximum": 4 },
            "response_format": { "type": "string", "enum": ["mp3", "pcm"] },
        }},
    }}})
}

/// The host's network: the fake OpenAI.
struct Network(Arc<FakeOpenAi>);

#[async_trait]
impl HttpClient for Network {
    async fn send(&self, request: HttpRequest) -> sidevoice_engine::Result<HttpResponse> {
        use std::sync::atomic::Ordering;
        let openai = &self.0;
        let key = request
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .map(|(_, value)| value.trim_start_matches("Bearer ").to_string())
            .unwrap_or_default();
        guard(&openai.asked).push((request.url.clone(), key.clone()));
        let answer =
            |status: u16, body: serde_json::Value| Ok(HttpResponse { status, body: body.to_string().into_bytes() });
        if request.url == OPENAI_SPEC {
            return answer(200, openai_spec());
        }
        if request.url != OPENAI_MODELS {
            return answer(404, serde_json::json!({}));
        }
        if key == "hold" {
            openai.holding.store(true, Ordering::SeqCst);
            while !openai.released.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        if key == "good" {
            return answer(200, serde_json::json!({ "data": [{ "id": "whisper-1" }, { "id": "tts-1" }] }));
        }
        if key.starts_with("sk-echo") {
            let said = format!("Incorrect API key provided: {key}.\nSee {}", "x".repeat(400));
            return answer(401, serde_json::json!({ "error": { "message": said } }));
        }
        if key == "scoped" {
            return answer(403, serde_json::json!({ "error": { "message": "Missing scopes: api.model.read" } }));
        }
        answer(401, serde_json::json!({ "error": { "message": "Incorrect API key provided." } }))
    }
}

/// The key the app keeps for OpenAI (the keychain's part), which a test changes.
#[derive(Default)]
struct KeptKey(Mutex<Option<String>>);

/// The host's keys: the one kept.
struct Keys(Arc<KeptKey>);

#[async_trait]
impl Credentials for Keys {
    async fn credential(&self, provider: &str) -> sidevoice_engine::Result<Option<String>> {
        Ok(if provider == "openai" { guard(&self.0 .0).clone() } else { None })
    }
}

struct ProviderHost {
    native: NativeHost,
    network: Network,
    keys: Keys,
}

impl Host for ProviderHost {
    fn capabilities(&self) -> sidevoice_engine::Capabilities {
        self.native.capabilities()
    }
    fn storage(&self) -> &dyn Storage {
        self.native.storage()
    }
    fn fetcher(&self) -> &dyn Fetcher {
        self.native.fetcher()
    }
    fn http(&self) -> &dyn HttpClient {
        &self.network
    }
    fn credentials(&self) -> &dyn Credentials {
        &self.keys
    }
}

/// The engines over a host whose OpenAI is fake, with `kept` the key the app keeps for it.
fn with_provider(name: &str, kept: Option<&str>) -> (Test, Arc<FakeOpenAi>, Arc<KeptKey>) {
    let root = std::env::temp_dir().join(format!("sidevoice-desktop-engine-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let openai = Arc::new(FakeOpenAi::default());
    let key = Arc::new(KeptKey(Mutex::new(kept.map(str::to_string))));
    let host = ProviderHost {
        native: NativeHost::new(&root).unwrap(),
        network: Network(Arc::clone(&openai)),
        keys: Keys(Arc::clone(&key)),
    };
    let engines = NativeEngines::with(Box::new(host), Vec::new(), runtime.handle().clone()).unwrap();
    (Test { engines, root, _runtime: runtime }, openai, key)
}

fn openai_view(engines: &NativeEngines) -> CatalogView {
    engines.catalogs().unwrap().into_iter().find(|view| view.id == "openai").unwrap()
}

fn code(refused: Option<CredentialRefusal>) -> Option<String> {
    refused.map(|refused| refused.reason.code)
}

/// A key tried is asked of the provider, and what the provider said is the answer; the key kept, and what was listed
/// with it, are as they were: nothing of the tried key is kept anywhere.
#[test]
fn a_key_tried_is_asked_of_the_provider_and_kept_nowhere() {
    let (test, openai, _) = with_provider("trial", Some("good"));
    let engines = &test.engines;
    let before = openai_view(engines);
    assert!(before.status.reason.is_none() && !before.models.is_empty(), "{before:?}");
    assert_eq!(openai.listed_with(), ["good"]);

    assert_eq!(code(engines.test_credential("openai", "bad").unwrap()).as_deref(), Some("credential-rejected"));
    assert_eq!(code(engines.test_credential("openai", "scoped").unwrap()).as_deref(), Some("listing-not-permitted"));
    assert_eq!(code(engines.test_credential("openai", " good ").unwrap()), None, "trimmed, and taken");
    assert_eq!(code(engines.test_credential("openai", "  ").unwrap()).as_deref(), Some("credential-missing"));
    assert_eq!(openai.listed_with(), ["good", "bad", "scoped", "good"], "each trial asked the provider itself");

    assert_eq!(openai_view(engines), before, "the kept listing is the one listed before");
    assert_eq!(openai.listed_with().len(), 4, "and was not listed again for it");

    assert_eq!(engines.test_credential("local", "x").unwrap_err().key, "catalog_not_found");
    assert_eq!(engines.test_credential("no-such", "x").unwrap_err().key, "catalog_not_found");
}

/// A refused key is told with the provider's own words, never with the key itself, and never longer than
/// `DETAIL_CHARS`; serialised, the detail sits beside `{code, params}`.
#[test]
fn a_refused_key_is_told_with_the_provider_words_without_the_key() {
    let (test, _, _) = with_provider("trial-detail", Some("good"));
    let engines = &test.engines;
    let refused = engines.test_credential("openai", "bad").unwrap().expect("refused");
    assert_eq!(refused.reason.code, "credential-rejected");
    assert!(refused.detail.as_deref().is_some_and(|said| said.contains("Incorrect API key provided")), "{refused:?}");
    let json = serde_json::to_value(&refused).unwrap();
    assert_eq!(json["code"], "credential-rejected");
    assert!(json.get("params").is_some() && json["detail"].is_string(), "{json}");

    let refused = engines.test_credential("openai", " sk-echo-SENTINEL-123 ").unwrap().expect("refused");
    let json = serde_json::to_string(&refused).unwrap();
    let said = refused.detail.expect("the provider's words");
    assert!(!said.contains("sk-echo-SENTINEL-123") && !json.contains("SENTINEL"), "{said}");
    assert!(said.contains("[key]") && !said.contains('\n'), "{said}");
    assert_eq!(said.chars().count(), catalogs::DETAIL_CHARS, "cut to the most, the last an ellipsis");
    assert!(said.ends_with('…'));
}

/// Words that are only blank are no detail, and a refusal without words has none: it serialises as `{code, params}`.
#[test]
fn a_refusal_without_words_has_no_detail() {
    let reason =
        || ReasonView { code: "credential-rejected".into(), params: catalogs::ReasonParams { needs: None, has: None } };
    assert_eq!(CredentialRefusal::new(reason(), Some(" \t\n "), "abc").detail, None);
    assert_eq!(
        CredentialRefusal::new(reason(), Some("bad abc, abc"), " abc ").detail.as_deref(),
        Some("bad [key], [key]")
    );
    let none = serde_json::to_value(CredentialRefusal::new(reason(), None, "abc")).unwrap();
    // The key as the provider may echo it: masked, cut short (the engine clips a body that is not its JSON), encoded,
    // in a header; none of it reaches the page, and words that merely look alike stay.
    let key = "sk-echo-SENTINEL-123";
    for echo in [
        "Incorrect API key provided: sk-echo-SENTINEL-***.",
        "Unauthorized: Bearer sk-echo-SENT",
        "bad key %73%6B-echo-SENTINEL-123",
        "key='sk-echo-SENTINEL-123'",
    ] {
        let said = CredentialRefusal::new(reason(), Some(echo), key).detail.unwrap();
        assert!(!said.contains("SENT") && !said.contains("echo-") && said.contains("[key]"), "{echo} → {said}");
    }
    assert_eq!(
        CredentialRefusal::new(reason(), Some("Incorrect API key provided."), key).detail.as_deref(),
        Some("Incorrect API key provided.")
    );
    let long = "é".repeat(400);
    let cut = CredentialRefusal::new(reason(), Some(&long), key).detail.unwrap();
    assert_eq!(cut.chars().count(), catalogs::DETAIL_CHARS);
    assert_eq!(none, serde_json::json!({ "code": "credential-rejected", "params": {} }));
}

/// A key the provider takes is not kept by trying it: with none kept, the provider still has none.
#[test]
fn a_key_taken_on_trial_is_not_kept() {
    let (test, openai, _) = with_provider("trial-none-kept", None);
    let engines = &test.engines;
    assert_eq!(code(engines.test_credential("openai", "good").unwrap()), None);
    let view = openai_view(engines);
    assert_eq!(view.status.reason.map(|r| r.code).as_deref(), Some("credential-missing"));
    assert!(view.models.is_empty());
    assert_eq!(openai.listed_with(), ["good"], "only the trial asked");
}

/// While a key is being tried, what the app lists — asked again or not — is listed with the kept key, and the trial
/// does not hold it up.
#[test]
fn a_listing_while_a_key_is_tried_is_the_kept_keys() {
    use std::sync::atomic::Ordering;
    /// Lets the held trial go however the test ends, so a failure fails rather than hangs.
    struct Release<'a>(&'a FakeOpenAi);
    impl Drop for Release<'_> {
        fn drop(&mut self) {
            self.0.released.store(true, Ordering::SeqCst);
        }
    }
    let (test, openai, _) = with_provider("trial-concurrent", Some("good"));
    let engines = &test.engines;
    std::thread::scope(|scope| {
        let release = Release(&openai);
        let trying = scope.spawn(|| engines.test_credential("openai", "hold"));
        let asked = std::time::Instant::now();
        while !openai.holding.load(Ordering::SeqCst) {
            assert!(asked.elapsed() < Duration::from_secs(10), "the trial never reached the provider");
            std::thread::sleep(Duration::from_millis(5));
        }
        // Listed with the trial held mid-request: on its own thread, so a listing the trial blocks fails the test.
        let (done, listed) = std::sync::mpsc::channel();
        scope.spawn(move || {
            let _ = done.send(engines.refresh("openai").map(|()| openai_view(engines)));
        });
        let view = listed.recv_timeout(Duration::from_secs(10)).expect("a listing waited for the trial").unwrap();
        assert!(view.status.reason.is_none() && !view.models.is_empty(), "{view:?}");
        drop(release);
        assert_eq!(code(trying.join().unwrap().unwrap()).as_deref(), Some("credential-rejected"));
    });
    assert_eq!(openai.listed_with(), ["hold", "good"], "the app's listing never carried the tried key");
    assert!(openai_view(engines).status.reason.is_none(), "nor did the trial's answer reach it");
}

/// `refresh` asks the provider again past the listing kept, with the key kept now.
#[test]
fn refresh_asks_the_provider_again_past_the_kept_listing() {
    let (test, openai, kept) = with_provider("refresh", Some("good"));
    let engines = &test.engines;
    openai_view(engines);
    openai_view(engines);
    assert_eq!(openai.listed_with(), ["good"], "a listing is kept");
    engines.refresh("openai").unwrap();
    assert_eq!(openai.listed_with(), ["good", "good"], "asked again");
    *guard(&kept.0) = Some("revoked".into());
    engines.refresh("openai").unwrap();
    let view = openai_view(engines);
    assert_eq!(view.status.reason.map(|r| r.code).as_deref(), Some("credential-rejected"));
    assert!(view.models.is_empty());
}
