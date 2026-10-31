//! The probe build and the native flows run on it (CI only, never shipped): the same app built with the `probe`
//! feature (src-tauri/src/probe.rs), which adds the probe page and the switch that loads it into the room window. Each
//! flow starts that app with a page or script that drives it and checks what the app, the page and the stand-ins log
//! (`SIDEVOICE_DEBUG=1` lines).

use std::fs;
use std::path::Path;
use std::time::Instant;

use crate::platform::file_mode::mode;
use crate::platform::macos_app::{app_command, binary, fake_node, release_target, AppHome, PROBE_ONLY};
use crate::platform::probe::checks::{card_frame, check_engine_line, check_voice_line, paths, proxy_port};
use crate::util::{
    contains, ensure, grep, has, interrupted, line_count, lines_after, re, read, read_bytes, repo, sleep, wait_for,
    work, Background, Cmd, TauriBuild,
};
use crate::Result;

/// `probe probe-app`: the app built with the `probe` feature, over the release app's place; it does carry the page
/// and its switch (so the release app's check for them means something).
pub fn probe_app() -> Result<()> {
    let build = TauriBuild {
        target: Some(release_target().triple),
        bundles: "app",
        features: Some("probe"),
        ..TauriBuild::default()
    };
    crate::platform::macos_app::unsigned(build.command()?).run()?;
    let bytes = read_bytes(&binary())?;
    for marker in PROBE_ONLY {
        ensure(contains(&bytes, marker), format!("the probe build does not carry {marker}"))?;
    }
    Ok(())
}

/// What a page cannot send to the local host's proxy, from outside the app, and what the proxy must answer: each
/// refused, none reaching the core (docs/LOCAL_HOST.md).
const REFUSALS: &[(&str, &str, &[&str])] = &[
    ("hostile-origin", "403", &["-H", "Origin: https://evil.example", "-H", "Authorization: Bearer x"]),
    ("no-origin", "403", &["-H", "Authorization: Bearer x"]),
    ("empty-origin", "403", &["-H", "Origin;", "-H", "Authorization: Bearer x"]),
    ("null-origin", "403", &["-H", "Origin: null", "-H", "Authorization: Bearer x"]),
    ("wrong-host", "421", &["-H", "Host: localhost:{port}", "-H", "Origin: tauri://localhost"]),
    ("rebound-host", "421", &["-H", "Host: evil.example:{port}", "-H", "Origin: tauri://localhost"]),
    ("no-secret", "401", &["-H", "Origin: tauri://localhost"]),
    (
        "guessed-secret",
        "401",
        &["-H", "Origin: tauri://localhost", "-H", "Authorization: Bearer AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"],
    ),
];

/// What the bundled page's run of test/fixtures/local-host-flow.js must report: running, the pairing object,
/// preflight → authenticated fetch with the token swapped in and its Origin kept, 401 without the secret, 404 for
/// native's own routes, a WebSocket spliced and refused.
const LOCAL_HOST_FACTS: &[&str] = &[
    "version=2",
    "state=running/api=1",
    "subscribed=running",
    "pairing=ok",
    "host=fake-machine",
    "fetch=200/device/tauri://localhost/the_bundled_page",
    "no_secret=401",
    "native_only=404",
    "ws=open/sidevoice/ping_from_the_page",
    "ws_no_secret=refused",
];

/// `probe local-host`: the local host from the bundled page, in the probe app: a stand-in core on its published socket
/// (`local-host-ci core`), pairing, the proxy, and what it refuses. A home of its own (/tmp/svhome), for the data
/// directory (macOS: ~/Library/Application Support/Sidevoice) and the app's config.
pub fn local_host() -> Result<()> {
    let app_dir = repo().join("src-tauri");
    Cmd::new("cargo")
        .args(["build", "--release", "-p", "sidevoice-desktop-test-support", "--bin", "local-host-ci"])
        .dir(&app_dir)
        .run()?;
    let tool = app_dir.join("target/release/local-host-ci");
    let home = Path::new("/tmp/svhome");
    crate::util::fresh_dir(home)?;
    let logs = work("local-host")?;
    let (core_log, app_log) = (logs.join("lh-core.log"), logs.join("lh.log"));
    let data = home.join("Library/Application Support/Sidevoice");
    let mut core = Cmd::new(&tool).arg("core").arg(&data).env("HOME", home).spawn(&core_log)?;
    for _ in 0..50 {
        if data.join("core/core.json").is_file() {
            break;
        }
        sleep(0.2);
    }
    let mut app = app_command(&[("SIDEVOICE_DEBUG_LOCAL_HOST_FLOW", "1")])
        .env("HOME", home)
        .env_remove("SIDEVOICE_DATA_DIR")
        .spawn(&app_log)?;
    wait_for(&app_log, "local-host-flow (ok|error)", 0, 120);
    let port = proxy_port(&read(&app_log)).unwrap_or_default();
    let mut failed = Vec::new();
    for (name, expected, headers) in REFUSALS {
        let mut curl = Cmd::new("curl").args(["-s", "-o", "/dev/null", "-w", "%{http_code}", "--max-time", "10"]);
        for header in *headers {
            curl = curl.arg(header.replace("{port}", &port));
        }
        let (_, got) = curl.arg(format!("http://127.0.0.1:{port}/api/presentation/echo")).attempt()?;
        eprintln!("{name}: {got}");
        if got != *expected {
            failed.push(format!("{name}={got}"));
        }
    }
    app.stop();
    sleep(1.0);
    core.stop();
    let (app_text, core_text) = (read(&app_log), read(&core_log));
    eprintln!("--- app\n{}", grep(&app_text, "local-host|proxy|page says").join("\n"));
    eprintln!("--- stand-in core\n{core_text}");
    ensure(failed.is_empty(), format!("not refused as the table says: {}", failed.join(" ")))?;
    let flow = grep(&app_text, "local-host-flow ok");
    ensure(!flow.is_empty(), "the bundled page's local-host flow did not report ok")?;
    for fact in LOCAL_HOST_FACTS {
        ensure(flow.iter().any(|line| line.contains(&format!(" {fact}"))), format!("missing {fact}"))?;
    }
    ensure(
        app_text.contains("proxy answered 204 OPTIONS /api/presentation/echo (preflight)"),
        "the proxy did not answer the preflight",
    )?;
    // Only the page's authenticated requests reached the core, each with the device token.
    let echoes = core_text.lines().filter(|line| line.contains("/api/presentation/echo")).count();
    ensure(echoes == 1, format!("the core saw {echoes} requests for /api/presentation/echo, not 1"))?;
    for request in [
        "fake-core POST /api/presentation/echo origin=tauri://localhost auth=device",
        "fake-core GET /api/browser/call origin=tauri://localhost auth=device",
    ] {
        ensure(core_text.contains(request), format!("the core never saw: {request}"))?;
    }
    ensure(!has(&core_text, "auth=other|/api/local/health origin=tauri"), "the core saw what it should not")?;
    // The token, stored by the app alone: 0600 in its config directory.
    let store = crate::platform::macos_app::app_data(home).join("local-host.json");
    let bits = mode(&store)?;
    ensure(bits == 0o600, format!("{} is {bits:o}, not 600", store.display()))
}

/// `probe probe`: the probe page in the interface's window, the same origin and permission rule as the room: a
/// secure context with WebCrypto, a node on loopback reached with a device token, no microphone for the page, the
/// native engine ([`check_engine_line`]) and its storage, the voice call ([`check_voice_line`]) and its settings,
/// and the headset's buttons.
pub fn probe() -> Result<()> {
    let data = AppHome::new("probe")?;
    let logs = work("probe")?;
    let (node_log, log) = (logs.join("probe-node.log"), logs.join("probe.log"));
    let mut node = fake_node(&node_log)?;
    let mut app = data.app(&[("SIDEVOICE_DEBUG_PAGE", "probe.html")]).spawn(&log)?;
    wait_for(&log, "probe-voice", 0, 480);
    app.stop();
    let text = read(&log);
    eprintln!("{text}");
    eprintln!("--- macOS {}", Cmd::new("sw_vers").arg("-productVersion").stdout().unwrap_or_default().trim());
    ensure(has(&text, "page tauri://localhost/probe.html"), "the probe page did not load")?;
    // Pairing needs a secure context and WebCrypto ECDSA P-256 on the app's own origin.
    ensure(has(&text, "probe origin=tauri://localhost secure=true ecdsa=true"), "no secure context with ECDSA P-256")?;
    // A local node (http://127.0.0.1:8768) reached from the app's origin with a device token: preflight + CORS.
    node.stop();
    ensure(text.contains("probe-loopback 200 Bearer probe-token"), "the page did not reach the node on loopback")?;
    ensure(
        read(&node_log).contains("request OPTIONS /api/presentation/echo origin=tauri://localhost"),
        "the node saw no preflight from the app's origin",
    )?;
    // The microphone is the app's here (host.voice): the page is never granted it.
    ensure(!has(&text, "sidevoice: media Microphone .*Allow"), "the page got the microphone")?;

    let engine = grep(&text, "probe-engine ok");
    ensure(!engine.is_empty(), "no probe-engine ok")?;
    check_engine_line(&engine.join("\n"))?;
    // sidevoice-engine's storage: a folder per installed build, nothing of the cancelled one, nothing half written.
    // The app's `Models` in the probe's home: `sidevoice-platform`'s `data_dir::models_dir()` on macOS. Spelled out because xtask
    // is a dependency-light package of its own (no C toolchain, no git dependency) and that crate pulls the local channel.
    let storage = data.home.join("Library/Application Support/Sidevoice/Models");
    Cmd::new("ls").arg("-laR").arg(storage.join("models")).run()?;
    let cancelled = paths(&storage.join("models")).into_iter().any(|p| p.to_string_lossy().contains("whisper-small"));
    ensure(!cancelled, "a cancelled download left files")?;
    let half = paths(&storage.join("partial")).into_iter().any(|p| p.is_file());
    ensure(!half, "a download was left half written")?;

    let voice = grep(&text, "probe-voice ok");
    ensure(!voice.is_empty(), "no probe-voice ok")?;
    let (build, turn) = check_voice_line(&voice.join("\n"))?;
    let settings = grep(&text, "sidevoice: voice settings: ");
    ensure(!settings.is_empty(), "the app logged no voice settings")?;
    // The call that listens and speaks is one of several the probe makes (a preload and a call that only speaks log
    // theirs): one of the choices made names the build and the end-of-turn model.
    ensure(
        settings.iter().any(|made| {
            made.contains(&format!(r#""stt":{{"catalog":"local","model":"whisper-tiny","build":"{build}"}}"#))
        }),
        format!("the voice settings do not name the build {build}"),
    )?;
    ensure(
        settings.iter().any(|made| made.contains(&format!(r#""end_of_turn":{{"catalog":"local","model":"{turn}"}}"#))),
        format!("the voice settings do not name the end-of-turn model {turn}"),
    )?;
    eprintln!("--- voice\n{}", grep(&text, "sidevoice: voice").join("\n"));
    // A dev build (the probe app is never a release): `host.build` says so, and there is no updater at all.
    let build = grep(&text, "probe-build ");
    eprintln!("--- build\n{}", build.join("\n"));
    ensure(
        build.iter().any(|line| {
            line.contains(r#"probe-build ok build={"channel":"dev","version":""#)
                && line.ends_with(" updater=false status=update_unavailable")
        }),
        "the probe app is not a dev build without an updater",
    )?;
    // The call settings through the app (the room page's own commands and grants), and no target.
    let settings = grep(&text, "probe-settings ");
    eprintln!("--- call settings\n{}", settings.join("\n"));
    ensure(
        settings.iter().any(|line| {
            re(r"probe-settings ok target=null set=Cmd\+Shift\+F9/(on|refused) invalid=shortcut_invalid off=off always=true/true$")
                .is_match(line)
        }),
        "the mute shortcut or the call controls settings did not answer as the seam says",
    )?;
    // Headset buttons: a (simulated) call made the app the Now Playing app, and it let go after.
    ensure(text.contains("probe-headset ok"), "no probe-headset ok")?;
    ensure(text.contains("headset apply active=true in_call=true mic=true"), "the call never took the headset")?;
    let applied = grep(&text, "headset apply");
    ensure(
        applied.last().is_some_and(|line| re("active=false in_call=false .* gesture_on=false").is_match(line)),
        "the app did not let go of the headset",
    )?;
    eprintln!("{}", applied.join("\n"));
    // The app's own mute is not mistaken for an AirPods gesture.
    ensure(!text.contains("headset airpods"), "echo recorded as a gesture")
}

/// `probe exit`: quitting and restarting after a call (#121). The probe app runs the probe page, which ends with
/// whisper-tiny's whisper.cpp build (Metal) loaded, then restarts as an installed update does
/// (`SIDEVOICE_DEBUG_EXIT=restart`, src-tauri/src/probe.rs); the relaunched app runs the page again and quits as the
/// tray's Quit does. Each process must release every model before `exit()`, then run the C++ static destructors to
/// their end (the probe's handler that runs after them, src-tauri/src/probe.rs), and end with status 0 and no crash
/// report.
pub fn exit() -> Result<()> {
    let data = AppHome::new("exit")?;
    let logs = work("exit")?;
    let log = logs.join("exit.log");
    // Where macOS writes them: the user's own home, whatever HOME the app was given.
    let reports = crate::platform::macos_app::home()?.join("Library/Logs/DiagnosticReports");
    let before: Vec<_> = crash_reports(&reports);
    let mut app =
        data.app(&[("SIDEVOICE_DEBUG_PAGE", "probe.html"), ("SIDEVOICE_DEBUG_EXIT", "restart")]).spawn(&log)?;
    let first = app.ended(480);
    if first.is_none() {
        app.stop();
    }
    // The relaunched app writes to the same log (it inherits the streams): its launch, its run, its quit.
    let relaunched = wait_for(&log, "sidevoice: probe: quitting", 0, 480);
    let mut gone = false;
    for _ in 0..60 {
        gone = !Cmd::new("pgrep").arg("-f").arg(binary()).attempt().map(|(found, _)| found).unwrap_or(false);
        if gone {
            break;
        }
        sleep(1.0);
    }
    if !gone {
        let _ = Cmd::new("pkill").arg("-f").arg(binary()).attempt();
    }
    // ReportCrash writes its report a moment after the process ends.
    sleep(10.0);
    let text = read(&log);
    eprintln!("{text}");
    let new: Vec<_> = crash_reports(&reports).into_iter().filter(|report| !before.contains(report)).collect();
    for report in &new {
        eprintln!("--- {}\n{}", report.display(), read(report).chars().take(6000).collect::<String>());
    }
    let launches = grep(&text, "sidevoice: launch ").len();
    eprintln!("--- first process: {first:?}; launches: {launches}; relaunched one quit: {relaunched}, gone: {gone}");
    ensure(
        grep(&text, "probe-exit models=ok build=whisper-tiny/").len() == 2,
        "a run did not load whisper.cpp's build",
    )?;
    ensure(first.is_some_and(|status| status.success()), format!("the app did not end cleanly on restart: {first:?}"))?;
    ensure(launches == 2, "the restart did not relaunch the app")?;
    ensure(relaunched && gone, "the relaunched app did not quit")?;
    ensure(!has(&text, "GGML_ASSERT|libc\\+\\+abi|panicked"), "the app aborted on its way out")?;
    // On a runner (Apple Paravirtual device) whisper.cpp's destructor does not abort with a model left loaded, unlike
    // on a Mac's GPU: what is checked is that none is left when `exit()` runs.
    let released = grep(&text, "sidevoice: voice: every model released").len();
    eprintln!("--- the release at exit\n{}", grep(&text, "sidevoice: voice: ").join("\n"));
    ensure(released == 2, format!("{released} of 2 processes released every model before exit()"))?;
    ensure(
        grep(&text, "sidevoice: probe: exit handlers ran").len() == 2,
        "exit() did not run the static destructors to their end in both processes",
    )?;
    ensure(new.is_empty(), "macOS wrote a crash report")
}

/// The crash reports of the app (`sidevoice-desktop-*.ips`) in `dir`.
fn crash_reports(dir: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("sidevoice-desktop")))
        .collect()
}

/// `probe voice-memory`: the host's calls on the engine's real models, the process's footprint and Metal allocation
/// taken at each step (src-tauri/src/voice/host/memory.rs, the ignored test): idle, a call with the models loaded, the
/// call ended and the idle keep run out, ten calls back to back, a call whose settings change twelve times. The table
/// is printed; the run fails only on clear growth (the checks are `probe::memory`). Reads `SIDEVOICE_REAL_CALL_MODELS` as
/// `voice-models` does.
pub fn voice_memory_log() -> Result<String> {
    let logs = work("voice-memory")?;
    let log = logs.join("voice-memory.log");
    let mut test = Cmd::new("cargo")
        .args(["test", "--release", "--lib", "voice::host::memory", "--", "--include-ignored", "--nocapture"])
        .dir(repo().join("src-tauri"))
        .spawn(&log)?;
    let status = test.ended(2400);
    let text = read(&log);
    eprintln!("{}", grep(&text, "memory phase=|test result|panicked").join("\n"));
    ensure(status.is_some_and(|status| status.success()), format!("the memory test did not pass: {status:?}"))?;
    Ok(text)
}

/// `probe memory`: the probe app through a long session (test/fixtures/memory.html): models loaded and let go after the
/// idle keep (shortened to two seconds a minute), ten calls, the call controls card's settings changed forty times,
/// the audio devices listed and chosen forty times and watched for forty seconds, the voice settings changed twenty
/// times. The app prints its own footprint and Metal allocation at each step (`probe-memory PHASE`); the table is
/// printed and the run fails only on clear growth (the checks are `probe::memory`).
pub fn memory_log() -> Result<String> {
    let data = AppHome::new("memory")?;
    let logs = work("memory")?;
    let log = logs.join("memory.log");
    let mut app =
        data.app(&[("SIDEVOICE_DEBUG_PAGE", "memory.html"), ("SIDEVOICE_DEBUG_IDLE_MINUTE_MS", "2000")]).spawn(&log)?;
    // The models install first (a download), then the walk; a step takes ten seconds or so.
    let done = memory_walk(&mut app, &log, &logs);
    app.stop();
    let text = read(&log);
    eprintln!("{}", grep(&text, "probe-memory|memory phase=|memory-holds").join("\n"));
    if !done {
        // Where the walk stopped: the app's last lines.
        eprintln!(
            "--- the app's last lines\n{}",
            text.lines().rev().take(80).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")
        );
    }
    ensure(!has(&text, "probe-memory-failed"), "the memory walk failed")?;
    ensure(done, "the memory walk never reached its last step")?;
    Ok(text)
}

/// The whole memory walk may take this long (a passing run takes about four and a half minutes with the models
/// installed first), and once it has begun no step takes this long to print the next `memory phase=` line (the longest
/// waits ten seconds and then reads the footprint).
const MEMORY_WALK_SECONDS: u64 = 360;
const MEMORY_STALL_SECONDS: u64 = 120;

/// Waits for the walk's last line. Bounded twice: as a whole, and by a stall (no new `memory phase=` line for
/// [`MEMORY_STALL_SECONDS`] once the walk has begun), so that a hung app is found in two minutes. Before giving up the
/// app's state goes to the log: its process line, and a `sample` of its threads (where each is waiting), which is what
/// tells a deadlock from a slow step (#27). Returns whether the walk finished.
fn memory_walk(app: &mut Background, log: &Path, logs: &Path) -> bool {
    let started = Instant::now();
    let mut seen = 0;
    let mut last = Instant::now();
    loop {
        let text = read(log);
        if has(&text, "memory phase=done ") {
            return true;
        }
        let phases = text.matches("memory phase=").count();
        if phases != seen {
            seen = phases;
            last = Instant::now();
        }
        if !app.running() {
            eprintln!("--- the app ended before the walk did");
            return false;
        }
        let stalled = seen > 0 && last.elapsed().as_secs() >= MEMORY_STALL_SECONDS;
        let late = started.elapsed().as_secs() >= MEMORY_WALK_SECONDS;
        if stalled || late || interrupted().is_err() {
            let why = if stalled { "no new step for 120 s" } else { "the walk is past 360 s" };
            eprintln!("--- {why} (after {} s, {seen} steps): the app's state", started.elapsed().as_secs());
            describe_hung(app.pid(), logs);
            return false;
        }
        sleep(1.0);
    }
}

/// What a hung process is doing, for the log: `ps` for its state, and macOS's `sample` of its threads for ten
/// seconds (the call stacks say which lock or main-thread call each waits on). Each is itself bounded.
fn describe_hung(pid: u32, logs: &Path) {
    let pid = pid.to_string();
    let state = Cmd::new("ps").args(["-o", "pid,stat,etime,time,%cpu,%mem,rss,command", "-p", &pid]).attempt();
    if let Ok((_, text)) = state {
        eprintln!("{text}");
    }
    if !cfg!(target_os = "macos") {
        return;
    }
    let file = logs.join("hung-sample.txt");
    let file_arg = file.to_string_lossy().into_owned();
    if let Ok(mut sample) =
        Cmd::new("sample").args([pid.as_str(), "10", "-file", file_arg.as_str()]).spawn(&logs.join("sample.log"))
    {
        if sample.ended(45).is_none() {
            eprintln!("--- sample did not end in 45 s");
            sample.stop();
        }
    }
    let sampled = read(&file);
    eprintln!(
        "--- sample of the app ({} lines, first 250)\n{}",
        sampled.lines().count(),
        sampled.lines().take(250).collect::<Vec<_>>().join("\n")
    );
}

/// The call controls card's run: the probe app on test/fixtures/card-probe.html, the editor of
/// test/fixtures/fullscreen-editor.swift, and test/fixtures/card-ci.swift to read the window server and move, click,
/// drag and type as a person.
struct Card {
    tool: std::path::PathBuf,
    log: std::path::PathBuf,
    editor_log: std::path::PathBuf,
    editor_app: std::path::PathBuf,
    shots: std::path::PathBuf,
    pid: String,
    editor: String,
    /// The card's window on screen (global points): x, y, width, height.
    at: Option<(i64, i64, i64, i64)>,
    failed: Vec<String>,
}

impl Card {
    fn fail(&mut self, what: impl Into<String>) {
        let what = what.into();
        eprintln!("::error::{what}");
        self.failed.push(what);
    }

    /// Waits for `pattern` in `log` after its first `after` lines; a failed check when it never shows.
    fn expect(&mut self, log: &Path, pattern: &str, after: usize) {
        if !wait_for(log, pattern, after, 60) {
            self.fail(format!("never saw in {}: {pattern}", log.display()));
        }
    }

    /// card-ci.swift: `move X Y`, `click X Y`, `drag X Y DX DY`, `type TEXT`.
    fn tool(&self, args: &[&str]) -> Result<()> {
        Cmd::new(&self.tool).args(args).run()
    }

    /// The pointer over the card, `dx`, `dy` from its top left corner.
    fn hover(&mut self, dx: i64, dy: i64) -> Result<()> {
        let (x, y, _, _) = self.at.unwrap_or_default();
        self.tool(&["move", &(x + dx).to_string(), &(y + dy).to_string()])
    }

    fn click(&mut self, x: i64, y: i64) -> Result<()> {
        self.tool(&["click", &x.to_string(), &y.to_string()])
    }

    fn shot(&self, name: &str) -> Result<()> {
        Cmd::new("screencapture").arg("-x").arg(self.shots.join(name)).run()
    }

    /// The card's window as the window server has it on screen: it must be in front of the editor.
    fn card(&mut self) {
        let windows = Cmd::new(&self.tool)
            .args(["windows", &self.pid, &self.editor])
            .attempt()
            .map(|(_, out)| out)
            .unwrap_or_default();
        eprint!("{windows}");
        if !windows.contains("card onscreen=true above_editor=true editor_onscreen=true") {
            self.fail(format!(
                "the card is not on screen in front of the editor: {}",
                windows.lines().next().unwrap_or("")
            ));
        }
        if let Some(frame) = card_frame(&windows) {
            self.at = Some(frame);
        }
    }

    fn editor_active(&mut self, when: &str) {
        let text = read(&self.editor_log);
        let last = grep(&text, "editor active=").last().map(|line| line.contains("active=true"));
        if last != Some(true) {
            self.fail(format!("the editor is not the active app ({when})"));
        }
    }
}

/// `probe card`: the call controls card over a full-screen editor: a non-activating panel that never takes the
/// keyboard from the editor, its controls on hover, its conversations on a click, dragged and remembered, its mute
/// and hang-up buttons clicked, and each of a call's states (screenshots into card-shots/). Every check runs; it
/// fails at the end with each that did not hold.
pub fn card() -> Result<()> {
    let home = AppHome::new("card")?;
    let dir = work("card")?;
    let shots = repo().join("card-shots");
    crate::util::mkdir(&shots)?;
    let tool = dir.join("card-ci");
    Cmd::new("swiftc").args(["-O", "-o"]).arg(&tool).arg("test/fixtures/card-ci.swift").run()?;
    // The editor as an app of its own, so LaunchServices opens and activates it as it would the person's.
    let editor_app = dir.join("Editor.app");
    crate::util::mkdir(&editor_app.join("Contents/MacOS"))?;
    Cmd::new("swiftc")
        .args(["-O", "-o"])
        .arg(editor_app.join("Contents/MacOS/Editor"))
        .arg("test/fixtures/fullscreen-editor.swift")
        .run()?;
    crate::util::write(
        &editor_app.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleExecutable</key><string>Editor</string><key>CFBundleIdentifier</key><string>ai.sidevoice.ci-editor</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleName</key><string>Editor</string></dict></plist>"#,
    )?;
    Cmd::new(&tool).arg("trusted").run()?;
    let mut card = Card {
        tool,
        log: dir.join("card.log"),
        editor_log: dir.join("editor.log"),
        editor_app,
        shots,
        pid: String::new(),
        editor: String::new(),
        at: None,
        failed: Vec::new(),
    };
    // A call walked through the card's states (test/fixtures/card-probe.html); this page answers its buttons. Each
    // line with the time, to order it against the editor's and the input's.
    let mut app = home.app(&[("SIDEVOICE_DEBUG_PAGE", "card-probe.html")]).spawn_timed(&card.log)?;
    card.pid = app.pid().to_string();
    let walked = walk(&mut card);
    app.stop();
    // By its binary, so it goes even when it never said its pid.
    let _ = Cmd::new("pkill").arg("-f").arg(card.editor_app.join("Contents/MacOS/Editor")).attempt();
    eprintln!(
        "--- app\n{}",
        grep(&read(&card.log), "call-controls|probe-card|(launch|ready|settled) app_active").join("\n")
    );
    eprintln!("--- editor\n{}", read(&card.editor_log));
    walked?;
    ensure(card.failed.is_empty(), format!("failed:\n{}", card.failed.join("\n")))
}

/// The card's walk; an error where the YAML stopped short (`exit 1`), with the failed checks so far.
fn walk(card: &mut Card) -> Result<()> {
    let stop = |card: &mut Card, what: &str| -> Result<()> {
        card.fail(what);
        Err(format!("failed:\n{}", card.failed.join("\n")))
    };
    let (log, editor_log) = (card.log.clone(), card.editor_log.clone());
    if !wait_for(&log, "probe-card phase=talking", 0, 60) {
        return stop(card, "the probe never started");
    }
    // The person's editor, full screen in its own Space: the room's window loses focus and the card shows over it, a
    // non-activating panel at the status level, on every Space and over full-screen apps, never the key window.
    crate::util::write(&editor_log, "")?;
    Cmd::new("open").arg("--stdout").arg(&editor_log).arg("--stderr").arg(&editor_log).arg(&card.editor_app).run()?;
    if !wait_for(&editor_log, "editor ready pid=", 0, 60) {
        return stop(card, "the editor never started");
    }
    card.editor =
        re(r"editor ready pid=([0-9]+)").captures(&read(&editor_log)).map(|c| c[1].to_owned()).unwrap_or_default();
    card.expect(&editor_log, "editor fullscreen=true key=true", 0);
    if !wait_for(&log, "call-controls shown", 0, 60) {
        return stop(card, "the card never showed");
    }
    // The policy at launch, before any window: the Dock tile is the Regular one's (#117).
    card.expect(&log, "launch app_active=(true|false) regular=true", 0);
    // With the card created, Launch Services (what the Dock and Cmd+Tab go by) must also have it as a regular app.
    card.expect(&log, "ready app_active=(true|false) regular=true policy=0 running_policy=0 .* call_card=on", 0);
    // What the system itself says, for the log: Launch Services' record of the process, and the Dock's tiles.
    for cmd in [
        Cmd::new("lsappinfo").arg("info").arg(format!("pid={}", card.pid)),
        Cmd::new("osascript")
            .arg("-e")
            .arg(r#"tell application "System Events" to get name of every UI element of list 1 of process "Dock""#),
    ] {
        match cmd.attempt() {
            Ok((ok, out)) => eprintln!("--- dock evidence (ok={ok})\n{out}"),
            Err(e) => eprintln!("--- dock evidence: {e}"),
        }
    }
    card.expect(&log, "call-controls panel ", 0);
    let panel = grep(&read(&log), "call-controls panel ").last().map(|l| l.to_string()).unwrap_or_default();
    if !re("nonactivating=true level=25 all_spaces=true fullscreen_auxiliary=true key=false visible=true .* app_active=false regular=true").is_match(&panel) {
        card.fail(format!("the panel is not what #4 asks, or the app is no longer a regular one (#117): {panel}"));
    }
    card.expect(&log, "call-controls watching clicks elsewhere: true", 0);
    card.expect(&log, "call-controls prevents activation: true", 0);
    sleep(3.0);
    card.card();
    if card.at.is_none() {
        return stop(card, "the card's window was never found");
    }
    card.shot("1-over-fullscreen-you-talking.png")?;
    // The card's page may not capture: it asked for the microphone (probe build) and was refused.
    card.expect(&log, r#"call-controls refused \{"capture":"[A-Za-z-]*","command":"probe-capture"\}"#, 0);
    let refused = grep(&read(&log), r#"call-controls refused \{"capture""#)
        .last()
        .map(|l| l.contains(r#""capture":"NotAllowedError""#));
    if refused != Some(true) {
        card.fail("the card's page was not refused the microphone");
    }
    if !read(&log).contains("call-controls permission Microphone denied") {
        card.fail("the app did not deny the card's request");
    }
    // The editor has the keyboard.
    card.tool(&["type", "abc"])?;
    card.expect(&editor_log, "editor text=abc", 0);

    // The pointer over the card (the app polls it: a panel that is never key gets no hover): its controls appear
    // below it, and the window grows to hold them.
    card.hover(160, 30)?;
    card.expect(&log, r"call-controls pointer inside=Some\(true\) app_active=false", 0);
    sleep(1.5);
    card.card();
    let h = card.at.unwrap_or_default().3;
    if h < 110 {
        card.fail(format!("the card did not grow with its controls (h={h})"));
    }
    card.shot("2-near-you-talking.png")?;

    // A click on the conversations chip (right of the title, the new card) opens them below the controls; a click in the editor closes it.
    let (x, y, _, _) = card.at.unwrap_or_default();
    card.click(x + 175, y + 37)?;
    sleep(1.5);
    card.card();
    let h = card.at.unwrap_or_default().3;
    if h < 200 {
        card.fail(format!("the conversations did not open (h={h})"));
    }
    card.shot("3-conversations.png")?;
    let from = line_count(&log);
    card.click(200, 700)?;
    card.expect(&log, "call-controls outside click", from);
    card.editor_active("after a click in it");

    // A drag from the card (not a button) moves it; let go, it is dropped and remembered. The app stays inactive.
    card.hover(160, 30)?;
    card.expect(&log, r"call-controls pointer inside=Some\(true\)", from);
    sleep(1.0);
    card.card();
    let before = card.at.map(|(x, y, _, _)| (x, y));
    let from = line_count(&log);
    let (x, y, _, _) = card.at.unwrap_or_default();
    card.tool(&["drag", &(x + 40).to_string(), &(y + 39).to_string(), "-300", "200"])?;
    card.expect(&log, "call-controls dropped at .* app_active=false", from);
    sleep(1.0);
    card.card();
    if card.at.map(|(x, y, _, _)| (x, y)) == before {
        card.fail("the card did not move");
    }
    card.shot("4-dragged.png")?;
    card.editor_active("after a drag");

    // Its mute button, clicked: the command reaches the room, the app never becomes active, and typing still reaches
    // the editor. Muted, then unmuted again (the probe then moves on).
    for state in ["muted", "unmuted"] {
        card.hover(160, 30)?;
        sleep(1.0);
        card.card();
        let from = line_count(&log);
        let (x, y, _, h) = card.at.unwrap_or_default();
        if state == "muted" {
            card.hovers_button(x + 156, y + h - 54, "mute", from)?;
        }
        card.click(x + 156, y + h - 54)?;
        card.expect(&log, "call-controls run toggle-mute app_active=false", from);
        card.expect(&log, "probe-card command toggle-mute", from);
        sleep(1.0);
        card.shot(&format!("5-{state}.png"))?;
        card.editor_active(&format!("after mute ({state})"));
    }
    card.tool(&["type", "def"])?;
    card.expect(&editor_log, "editor text=abc.*def", 0);
    card.editor_active("at the end of the clicks");

    // The other states, near (controls) and away (at rest).
    card.expect(&log, "probe-card phase=working", 0);
    sleep(2.0);
    card.shot("6-near-agent-working.png")?;
    card.expect(&log, "probe-card phase=speaking", 0);
    sleep(2.0);
    card.shot("7-near-agent-speaking.png")?;
    let from = line_count(&log);
    card.move_to(200, 700)?;
    card.expect(&log, r"call-controls pointer inside=Some\(false\) app_active=false", from);
    card.expect(&log, "probe-card phase=muted", 0);
    sleep(2.0);
    card.shot("8-rest-muted-agent-speaking.png")?;
    card.expect(&log, "probe-card phase=reconnecting", 0);
    sleep(2.0);
    card.shot("9-rest-reconnecting.png")?;
    // Its hang-up button ends the call; the card goes with it (after that, not from an earlier hide).
    if read(&log).contains("call-controls hidden") {
        card.fail("the card hid before the call ended");
    }
    card.card();
    card.hover(160, 30)?;
    sleep(1.5);
    card.card();
    let from = line_count(&log);
    let (x, y, w, h) = card.at.unwrap_or_default();
    card.click(x + w - 122, y + h - 54)?;
    card.expect(&log, "probe-card command hang-up", from);
    let text = read(&log);
    let hung_up = lines_after(&text, 0)
        .enumerate()
        .filter(|(_, line)| line.contains("probe-card command hang-up"))
        .map(|(at, _)| at + 1)
        .last()
        .unwrap_or(from);
    card.expect(&log, "call-controls hidden", hung_up);
    card.expect(&log, "probe-card done", from);
    let editor_text = read(&editor_log);
    if editor_text.contains("editor active=false") {
        card.fail("the editor stopped being the active app");
    }
    // Nor the keyboard: its window stays key from full screen on (a loss put right later still counts).
    let full_screen = editor_text.lines().skip_while(|line| !line.contains("editor fullscreen=true key=true"));
    if full_screen.into_iter().any(|line| line.contains("editor key=false")) {
        let lost = editor_text.lines().find(|line| line.contains("editor key=false")).unwrap_or("");
        card.fail(format!("the editor's window lost the keyboard: {lost}"));
    }
    Ok(())
}

impl Card {
    /// Hover with no click (#139): the pointer moved onto a button as a mouse does, the card's page sees `:hover` on it
    /// with the hand cursor (its probe script says so), and the screen shows the hand, all while the editor stays the
    /// active app. Before anything clicks there: a click would set `:hover` even where the page gets no mouse moves, so
    /// a new hovered button logged after `from`, with no click since, can only come from the moves. Any button: its
    /// class names are the web build's to change.
    fn hovers_button(&mut self, x: i64, y: i64, button: &str, from: usize) -> Result<()> {
        self.tool(&["hover", &x.to_string(), &y.to_string()])?;
        let log = self.log.clone();
        self.expect(
            &log,
            r#"call-controls refused \{"button":"[^"]+","command":"probe-hover","cursor":"pointer"\}"#,
            from,
        );
        let cursor = Cmd::new(&self.tool).arg("cursor").attempt().map(|(_, out)| out).unwrap_or_default();
        eprint!("{cursor}");
        if !cursor.contains("cursor hand=true") {
            self.fail(format!("the hand cursor did not show over {button}: {}", cursor.trim()));
        }
        self.editor_active(&format!("after hovering {button}"));
        Ok(())
    }

    /// The pointer to a point on screen (away from the card).
    fn move_to(&mut self, x: i64, y: i64) -> Result<()> {
        self.tool(&["move", &x.to_string(), &y.to_string()])
    }
}
