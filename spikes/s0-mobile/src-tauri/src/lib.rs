//! S0 spike (sidevoice-desktop#290, method: architecture-v3.md §F.1): does the room page stay alive on a locked phone
//! while the audio is native? A scratch Tauri 2 mobile app, never merged.
//!
//! - The launcher (`dist/index.html`, from `launcher/`) starts a run: **N** (native-held: the holder plugin holds the
//!   audio, page media is disabled and the page's voice is a diagnostic adapter fed by native synthetic events) or
//!   **P** (page-audio control: the holder stopped, the page's own browser voice path).
//! - Everything is logged natively, timestamped by this process, to `<log dir>/s0/<run>/app.jsonl`: what the page
//!   reports through the bridge (`s0_log`: its 1 s timer, its socket and wire, its media counters), this process's own
//!   5 s liveness tick, and the synthetic events it emits. The holder writes `holder.jsonl`, the system watch
//!   `system.jsonl`, next to it.
//! - The window keeps `BackgroundThrottlingPolicy::Disabled`.
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Map, Value};
use tauri::{utils::config::BackgroundThrottlingPolicy, AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_s0_holder::HolderExt;

/// The page's instrumentation and run N's diagnostic voice adapter (`s0-init.js`), on every page of the window.
const INIT: &str = include_str!("s0-init.js");
const NATIVE_TICK: Duration = Duration::from_secs(5);

struct Run {
    id: String,
    mode: String,
    dir: PathBuf,
    log: File,
    started: Instant,
    synthetic_every: Duration,
    generation: u64,
}

/// The run in progress, process-wide: an Android activity made again (task removal and relaunch with the service
/// alive) finds it and goes on writing to the same file.
#[derive(Default)]
struct Spike {
    run: Mutex<Option<Run>>,
    app: Mutex<Option<AppHandle>>,
    generation: Mutex<u64>,
}

fn spike() -> &'static Spike {
    static SPIKE: OnceLock<Spike> = OnceLock::new();
    SPIKE.get_or_init(Spike::default)
}

fn wall_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn write(run: &mut Run, src: &str, kind: &str, data: Value) {
    let mut line = match data {
        Value::Object(map) => map,
        Value::Null => Map::new(),
        other => {
            let mut map = Map::new();
            map.insert("data".into(), other);
            map
        }
    };
    line.insert("t".into(), json!(wall_ms()));
    line.insert("mono".into(), json!(run.started.elapsed().as_millis() as u64));
    line.insert("src".into(), json!(src));
    line.insert("k".into(), json!(kind));
    let _ = writeln!(run.log, "{}", Value::Object(line));
    let _ = run.log.flush();
}

fn log(src: &str, kind: &str, data: Value) {
    if let Some(run) = spike().run.lock().unwrap().as_mut() {
        write(run, src, kind, data);
    }
}

fn app() -> Option<AppHandle> {
    spike().app.lock().unwrap().clone()
}

/// This process's own clock, while a run lasts: a liveness line every 5 s, and in run N a synthetic event to the page
/// every `synthetic_every`, labelled `synthetic`, which the page's diagnostic adapter turns into a turn.
fn native_clock(generation: u64) {
    let mut last_tick = Instant::now();
    let mut last_synthetic = Instant::now();
    let mut seq = 0u64;
    loop {
        thread::sleep(Duration::from_millis(250));
        let (mode, every) = {
            let guard = spike().run.lock().unwrap();
            match guard.as_ref() {
                Some(run) if run.generation == generation => (run.mode.clone(), run.synthetic_every),
                _ => return,
            }
        };
        if last_tick.elapsed() >= NATIVE_TICK {
            last_tick = Instant::now();
            let holder = app().and_then(|app| app.holder().status().ok());
            log("native", "native-tick", json!({ "holder": holder }));
        }
        if mode == "N" && last_synthetic.elapsed() >= every {
            last_synthetic = Instant::now();
            seq += 1;
            let at = wall_ms();
            let delivered = app()
                .and_then(|app| app.get_webview_window("main"))
                .map(|window| {
                    window
                        .eval(format!("window.__s0&&window.__s0.synthetic({{seq:{seq},t:{at}}})"))
                        .is_ok()
                })
                .unwrap_or(false);
            log("native", "synthetic-emitted", json!({ "seq": seq, "at": at, "eval": delivered, "synthetic": true }));
        }
    }
}

fn end_run(app: &AppHandle, reason: &str) {
    let ended = spike().run.lock().unwrap().take();
    if let Some(mut run) = ended {
        let holder = app.holder().stop();
        let _ = app.holder().unwatch();
        write(&mut run, "native", "run-end", json!({ "reason": reason, "holder_stop": format!("{holder:?}") }));
    }
}

#[tauri::command]
async fn s0_begin(app: AppHandle, mode: String, synthetic_every: Option<u64>, device: Option<Value>) -> Result<Value, String> {
    if mode != "N" && mode != "P" {
        return Err(format!("unknown mode {mode}"));
    }
    let worker = app.clone();
    tauri::async_runtime::spawn_blocking(move || begin(&worker, mode, synthetic_every.unwrap_or(60), device))
        .await
        .map_err(|e| e.to_string())?
}

fn begin(app: &AppHandle, mode: String, synthetic_every: u64, device: Option<Value>) -> Result<Value, String> {
    end_run(app, "replaced");
    let base = PathBuf::from(app.holder().log_dir()?).join("s0");
    let id = format!("{mode}-{}", wall_ms() / 1000);
    let dir = base.join(&id);
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("app.jsonl"))
        .map_err(|e| e.to_string())?;
    let generation = {
        let mut generation = spike().generation.lock().unwrap();
        *generation += 1;
        *generation
    };
    let mut run = Run {
        id: id.clone(),
        mode: mode.clone(),
        dir: dir.clone(),
        log,
        started: Instant::now(),
        synthetic_every: Duration::from_secs(synthetic_every.max(10)),
        generation,
    };
    write(
        &mut run,
        "native",
        "run-begin",
        json!({ "id": id, "mode": mode, "synthetic_every_s": synthetic_every.max(10), "device": device,
                "app": app.package_info().version.to_string(), "tauri": tauri::VERSION }),
    );
    let dir_text = dir.to_string_lossy().into_owned();
    let watch = app.holder().watch(&dir_text);
    write(&mut run, "native", "system-watch", json!({ "result": format!("{watch:?}") }));
    let holder = if mode == "N" {
        app.holder().start(&dir_text)
    } else {
        app.holder().stop()
    };
    let holder_ok = holder.is_ok();
    write(&mut run, "native", if mode == "N" { "holder-started" } else { "holder-stopped" }, json!({ "result": format!("{holder:?}") }));
    *spike().run.lock().unwrap() = Some(run);
    if mode == "N" && !holder_ok {
        end_run(app, "holder-failed");
        return Err(format!("the native holder did not start: {holder:?}"));
    }
    thread::spawn(move || native_clock(generation));
    Ok(json!({ "id": id, "mode": mode, "dir": dir_text }))
}

#[tauri::command]
async fn s0_end(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || end_run(&app, "ended"))
        .await
        .map_err(|e| e.to_string())
}

/// One thing the page reports, written with this process's clock.
#[tauri::command]
fn s0_log(entry: Value) {
    let kind = entry.get("k").and_then(Value::as_str).unwrap_or("page").to_owned();
    log("page", &kind, entry);
}

#[tauri::command]
async fn s0_status(app: AppHandle) -> Value {
    let run = spike().run.lock().unwrap().as_ref().map(|run| {
        json!({ "id": run.id, "mode": run.mode, "dir": run.dir.to_string_lossy(), "elapsed_s": run.started.elapsed().as_secs() })
    });
    let holder = tauri::async_runtime::spawn_blocking(move || app.holder().status().ok()).await.ok().flatten();
    json!({ "run": run, "holder": holder })
}

fn runs_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(PathBuf::from(app.holder().log_dir()?).join("s0"))
}

/// The runs on this phone, newest first, with their files' sizes.
#[tauri::command]
async fn s0_runs(app: AppHandle) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let base = runs_dir(&app)?;
        let mut runs = Vec::new();
        for entry in fs::read_dir(&base).into_iter().flatten().flatten() {
            let files: Map<String, Value> = fs::read_dir(entry.path())
                .into_iter()
                .flatten()
                .flatten()
                .map(|f| (f.file_name().to_string_lossy().into_owned(), json!(f.metadata().map(|m| m.len()).unwrap_or(0))))
                .collect();
            runs.push(json!({ "id": entry.file_name().to_string_lossy(), "files": files }));
        }
        runs.sort_by(|a, b| b["id"].as_str().cmp(&a["id"].as_str()));
        Ok(Value::Array(runs))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A run's files, as text, for the launcher to send to the paired machine.
#[tauri::command]
async fn s0_read(app: AppHandle, run: String) -> Result<Value, String> {
    if run.contains('/') || run.contains('\\') || run.starts_with('.') {
        return Err("not a run".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let dir = runs_dir(&app)?.join(&run);
        let mut files = Map::new();
        for f in fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
            let text = fs::read_to_string(f.path()).map_err(|e| e.to_string())?;
            files.insert(f.file_name().to_string_lossy().into_owned(), json!(text));
        }
        Ok(Value::Object(files))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_s0_holder::init())
        .invoke_handler(tauri::generate_handler![s0_begin, s0_end, s0_log, s0_status, s0_runs, s0_read])
        .setup(|app| {
            *spike().app.lock().unwrap() = Some(app.handle().clone());
            // A relaunch during a run (Android: the task removed, the service alive) goes back to the room page.
            let resumed = spike().run.lock().unwrap().is_some();
            let page = if resumed { "voice/" } else { "index.html" };
            log("native", "window-create", json!({ "page": page }));
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App(page.into()))
                .initialization_script(INIT)
                .background_throttling(BackgroundThrottlingPolicy::Disabled)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the S0 spike");
}
