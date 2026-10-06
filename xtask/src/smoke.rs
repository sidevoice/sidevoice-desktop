//! `smoke` (macOS): the app's native flows, in a build with the `probe` feature (src-tauri/src/probe.rs; never
//! shipped). Each flow starts the app with a scratch home and a page or script that drives it, then checks what the
//! app, the page and the stand-ins wrote (`SIDEVOICE_DEBUG=1` lines). Every check of a flow runs; the flow fails at
//! the end with each that did not hold.

use crate::macos::{self, app_command, app_dir, binary, contains, fake_node, scratch_home, PROBE_ONLY};
use crate::util::{
    has, line_count, lines_matching, number_after, output, read, repo, run, sleep, target, wait_for, word_after,
    Background, Checks,
};
use crate::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn smoke() -> Result<()> {
    let logs = target().join("xtask");
    fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
    engine_round_trip()?;
    probe_build()?;
    let mut failed = Vec::new();
    for (name, flow) in [
        ("local host", local_host as fn() -> Result<()>),
        ("probe page", probe_page),
        ("call controls card", card),
        ("room flow", room_flow),
    ] {
        eprintln!("=== {name}");
        if let Err(error) = flow() {
            eprintln!("::error::{error}");
            failed.push(name);
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("smoke flows failed: {}", failed.join(", ")))
    }
}

fn logs(name: &str) -> PathBuf {
    target().join("xtask").join(name)
}

/// The native engine on its own: download, then Kokoro says a sentence and Whisper hears it, on the accelerator the
/// resolver picks (CPU); with Core ML too, for the record only.
fn engine_round_trip() -> Result<()> {
    let engines = target().join("xtask/engines");
    let round_trip = |extra: &[&str]| {
        run(Command::new("cargo")
            .args(["run", "--locked", "--release", "-p", "sidevoice-desktop-engine", "--example", "roundtrip", "--"])
            .arg(&engines)
            .args(extra)
            .current_dir(repo()))
    };
    round_trip(&[])?;
    if let Err(error) = round_trip(&["coreml"]) {
        eprintln!("::warning::round trip with Core ML (informational): {error}");
    }
    Ok(())
}

/// The probe build, which does carry the page and its switches: what the release check looks for is real.
fn probe_build() -> Result<()> {
    macos::build(Some("probe"))?;
    let bytes = fs::read(binary()).map_err(|e| e.to_string())?;
    let missing: Vec<&str> = PROBE_ONLY.iter().copied().filter(|marker| !contains(&bytes, marker)).collect();
    missing.is_empty().then_some(()).ok_or_else(|| format!("the probe build lacks {missing:?}"))
}

/// The local host from the bundled page: a stand-in core on `local.sock`, pairing, the proxy, and its refusals.
fn local_host() -> Result<()> {
    run(Command::new("cargo")
        .args(["build", "--locked", "--release", "-p", "sidevoice-local-host", "--example", "local-host-ci"])
        .current_dir(repo()))?;
    let tool = target().join("release/examples/local-host-ci");
    let home = scratch_home("local-host")?;
    let data = home.join(".sidevoice");
    let (core_log, app_log) = (logs("lh-core.log"), logs("lh.log"));
    let mut core = Background::start(Command::new(&tool).arg("core").arg(&data), &core_log)?;
    for _ in 0..50 {
        if data.join("core/local.sock").exists() {
            break;
        }
        sleep(0.2);
    }
    let mut app = Background::start(&mut app_command(&home, &[("SIDEVOICE_DEBUG_LOCAL_HOST_FLOW", "1")]), &app_log)?;
    for _ in 0..240 {
        if has(&read(&app_log), "local-host-flow ok") || has(&read(&app_log), "local-host-flow error") {
            break;
        }
        sleep(0.5);
    }
    let port = read(&app_log)
        .lines()
        .find_map(|line| line.split("local-host proxy on http://127.0.0.1:").nth(1).map(str::to_owned))
        .map(|rest| rest.chars().take_while(char::is_ascii_digit).collect::<String>())
        .ok_or("the app never said where its proxy listens")?;

    // What a page cannot send, from outside the app: each refused by the proxy, none reaching the core.
    let mut checks = Checks::default();
    let url = format!("http://127.0.0.1:{port}/api/presentation/echo");
    let refused: &[(&str, &str, &[&str])] = &[
        ("hostile-origin", "403", &["Origin: https://evil.example", "Authorization: Bearer x"]),
        ("no-origin", "403", &["Authorization: Bearer x"]),
        ("empty-origin", "403", &["Origin;", "Authorization: Bearer x"]),
        ("null-origin", "403", &["Origin: null", "Authorization: Bearer x"]),
        ("wrong-host", "421", &["Host: localhost:{port}", "Origin: tauri://localhost"]),
        ("rebound-host", "421", &["Host: evil.example:{port}", "Origin: tauri://localhost"]),
        ("no-secret", "401", &["Origin: tauri://localhost"]),
        (
            "guessed-secret",
            "401",
            &["Origin: tauri://localhost", "Authorization: Bearer AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"],
        ),
    ];
    for (name, expected, headers) in refused {
        let mut curl = Command::new("curl");
        curl.args(["-s", "-o", "/dev/null", "-w", "%{http_code}", "--max-time", "10"]);
        for header in *headers {
            curl.arg("-H").arg(header.replace("{port}", &port));
        }
        let got = output(curl.arg(&url)).unwrap_or_else(|e| e);
        eprintln!("{name}: {got}");
        checks.check(got.trim() == *expected, format!("{name}: {got} (expected {expected})"));
    }
    app.stop();
    sleep(1.0);
    core.stop();
    let (app_text, core_text) = (read(&app_log), read(&core_log));
    eprintln!("--- app");
    for line in app_text.lines().filter(|l| ["local-host", "proxy", "page says"].iter().any(|k| l.contains(k))) {
        eprintln!("{line}");
    }
    eprintln!("--- stand-in core\n{core_text}");

    // The bundled page: running, the pairing object, preflight → authenticated fetch with the token swapped in and its
    // Origin kept, 401 without the secret, 404 for native's own routes, a WebSocket spliced and refused.
    let line = app_text.lines().find(|l| l.contains("local-host-flow ok")).unwrap_or("");
    checks.check(!line.is_empty(), "the page's local-host flow did not pass");
    for fact in [
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
    ] {
        checks.check(line.contains(&format!(" {fact}")), format!("the local-host flow lacks {fact}"));
    }
    checks.check(
        has(&app_text, "proxy answered 204 OPTIONS /api/presentation/echo (preflight)"),
        "the proxy did not answer the preflight",
    );
    // Only the page's authenticated requests reached the core, each with the device token.
    let echoes = core_text.lines().filter(|l| l.contains("/api/presentation/echo")).count();
    checks.check(echoes == 1, format!("the core saw {echoes} echo requests (expected 1)"));
    for seen in [
        "fake-core POST /api/presentation/echo origin=tauri://localhost auth=device",
        "fake-core GET /api/browser/call origin=tauri://localhost auth=device",
    ] {
        checks.check(has(&core_text, seen), format!("the core never saw: {seen}"));
    }
    checks.check(
        !has(&core_text, "auth=other") && !has(&core_text, "/api/local/health origin=tauri"),
        "the core saw what it should not",
    );
    // The token, stored by the app alone: 0600 in its config directory.
    let store = app_dir(&home).join("local-host.json");
    checks.check(mode(&store) == Some(0o600), format!("{} is not 0600", store.display()));
    checks.done("local host")
}

fn mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).ok().map(|m| m.permissions().mode() & 0o777)
}

fn models(home: &Path) -> PathBuf {
    app_dir(home).join("engines/models/sherpa-onnx")
}

/// The models on disk, listed; whether a cancelled download (whisper-small) left anything.
fn cancelled_left_files(home: &Path) -> bool {
    let dir = models(home);
    eprintln!("{}", output(Command::new("ls").arg("-la").arg(&dir)).unwrap_or_else(|e| e));
    fs::read_dir(&dir)
        .map(|entries| entries.flatten().any(|e| e.file_name().to_string_lossy().contains("whisper-small")))
        .unwrap_or(false)
}

/// What the interface's window offers: the probe page (test/fixtures/probe.html) on the app's origin, with the same
/// permission rule: secure context, a loopback node through CORS, the native engine through the bridge, headset.
fn probe_page() -> Result<()> {
    let home = scratch_home("probe")?;
    let (app_log, node_log) = (logs("probe.log"), logs("probe-node.log"));
    let mut node = fake_node(&node_log)?;
    // A model stays in memory 5 s unused here (10 minutes in a release), so the probe sees D13 happen.
    let mut app = Background::start(
        &mut app_command(&home, &[("SIDEVOICE_DEBUG_PAGE", "probe.html"), ("SIDEVOICE_DEBUG_IDLE_UNLOAD_SECS", "5")]),
        &app_log,
    )?;
    wait_for(&app_log, "probe-engine", 0, 240);
    app.stop();
    node.stop();
    let text = read(&app_log);
    let node_text = read(&node_log);
    eprintln!("{text}");
    eprintln!("--- macOS {}", output(Command::new("sw_vers").arg("-productVersion")).unwrap_or_default().trim());

    let mut c = Checks::default();
    c.check(has(&text, "page tauri://localhost/probe.html"), "the probe page did not load");
    // Pairing needs a secure context and WebCrypto ECDSA P-256 on the app's own origin.
    c.check(has(&text, "probe origin=tauri://localhost secure=true ecdsa=true"), "no secure context with ECDSA");
    // A local node reached from the app's origin with a device token: preflight + CORS.
    c.check(has(&text, "probe-loopback 200 Bearer probe-token"), "the loopback node was not reached with the token");
    c.check(
        has(&node_text, "request OPTIONS /api/presentation/echo origin=tauri://localhost"),
        "the node saw no preflight from the app's origin",
    );
    match lines_matching(&text, "sidevoice: media", 0).next() {
        Some(line) => eprintln!("{line}"),
        None => eprintln!("(WebKit did not ask the app about the microphone)"),
    }

    // The native engine inside the signed app, through the bridge the interface uses (docs/BRIDGE.md).
    let ok: Vec<&str> = lines_matching(&text, "probe-engine ok", 0).collect();
    let any = |check: &dyn Fn(&str) -> bool| ok.iter().any(|line| check(line));
    let after_ok = |line: &str| line.split_once("probe-engine ok").map(|(_, rest)| rest.to_owned()).unwrap_or_default();
    c.check(!ok.is_empty(), "the probe's engine checks did not pass");
    c.check(
        any(&|l| {
            l.contains(
                r#"probe-engine ok capabilities={"runs":"native","os":"macos","arch":"aarch64","has":["cpu","coreml""#,
            )
        }),
        "capabilities are not native macOS arm64 with cpu and coreml",
    );
    c.check(any(&|l| at_least(&after_ok(l), r#""memory_mb":"#, 1000)), "no real memory_mb");
    for fact in [
        " installed=kokoro-82m-v1.0@sherpa-onnx,whisper-tiny@sherpa-onnx ",
        " refused=yes ",
        " before_load=kokoro-82m-v1.0@sherpa-onnx/cpu load_ms=",
        " loaded=kokoro-82m-v1.0@sherpa-onnx/cpu,whisper-tiny@sherpa-onnx/cpu since_ok=true same_models=true unloaded=kokoro-82m-v1.0@sherpa-onnx/cpu ",
        " preloaded=kokoro-82m-v1.0@sherpa-onnx/cpu,whisper-tiny@sherpa-onnx/cpu ",
        " cancel=true/install_cancelled/false cancel_gone=true progress_event=bytes_per_s+done+engine+job+model+total:true ",
    ] {
        c.check(any(&|l| after_ok(l).contains(fact)), format!("probe-engine lacks{fact}"));
    }
    c.check(any(&|l| at_least(&after_ok(l), " progress_calls=", 1)), "no download progress was reported");
    c.check(any(&|l| after_ok(l).to_lowercase().contains("prueba")), "Whisper did not hear the Spanish sentence");
    // Loaded once: a second load reports the first load's time.
    c.check(any(&|l| loaded_once(&after_ok(l))), "a loaded model was loaded again");
    c.check(any(&|l| memory_reported(&after_ok(l))), "no memory {total_mb, available_mb}");
    c.check(
        any(&|l| {
            word_between(&after_ok(l), "text_en=\"", '"')
                .is_some_and(|t| t.to_lowercase().contains("test") || t.to_lowercase().contains("voice"))
        }),
        "Whisper did not hear the English sentence",
    );
    // D13: after the call, unused for the idle time, both unloaded by the app; loaded again as the next call connects.
    for line in [
        "sidevoice: engine unloaded idle whisper-tiny@sherpa-onnx/cpu",
        "sidevoice: engine unloaded idle kokoro-82m-v1.0@sherpa-onnx/cpu",
        "sidevoice: engine preload whisper-tiny@sherpa-onnx/cpu load_ms=",
    ] {
        c.check(has(&text, line), format!("never saw: {line}"));
    }
    c.check(
        any(&|l| number_after(&after_ok(l), " speed=").is_some_and(|(n, rest)| n >= 1 && rest.starts_with(' '))),
        "no download speed",
    );
    c.check(!cancelled_left_files(&home), "a cancelled download left files");

    // Headset buttons: a (simulated) call made the app the Now Playing app, and it let go after.
    c.check(has(&text, "probe-headset ok"), "the headset probe did not pass");
    c.check(has(&text, "headset apply active=true in_call=true mic=true"), "the call never took the headset");
    let applies: Vec<&str> = lines_matching(&text, "headset apply", 0).collect();
    for line in &applies {
        eprintln!("{line}");
    }
    c.check(
        applies.last().is_some_and(|l| crate::util::matches(l, "active=false in_call=false .* gesture_on=false")),
        "the app did not let go of the headset after the call",
    );
    // The app's own mute is not mistaken for an AirPods gesture.
    c.check(!has(&text, "headset airpods"), "echo recorded as a gesture");
    c.done("probe page")
}

/// Whether the number after `key` in `text` is at least `floor`.
fn at_least(text: &str, key: &str, floor: u64) -> bool {
    number_after(text, key).is_some_and(|(n, _)| n >= floor)
}

/// ` load_ms=N load_again_ms=N `.
fn loaded_once(text: &str) -> bool {
    text.match_indices(" load_ms=").any(|(at, _)| {
        let Some((first, rest)) = number_after(&text[at..], " load_ms=") else { return false };
        number_after(rest, " load_again_ms=")
            .is_some_and(|(again, tail)| rest.starts_with(" load_again_ms=") && again == first && tail.starts_with(' '))
    })
}

/// `memory={"total_mb":N,"available_mb":M}`, N ≥ 1000, M ≥ 1.
fn memory_reported(text: &str) -> bool {
    let Some((total, rest)) = number_after(text, r#"memory={"total_mb":"#) else { return false };
    let Some((available, tail)) = number_after(rest, r#","available_mb":"#) else { return false };
    total >= 1000 && rest.starts_with(r#","available_mb":"#) && available >= 1 && tail.starts_with('}')
}

/// What follows `key` up to `end`.
fn word_between<'a>(text: &'a str, key: &str, end: char) -> Option<&'a str> {
    let at = text.find(key)? + key.len();
    text[at..].split(end).next()
}

/// The call controls card over a full-screen editor: never activating, its clicks, a drag, typing kept by the editor,
/// and the call's states (test/fixtures/card-probe.html walks a call through them; card-ci.swift is the hand).
fn card() -> Result<()> {
    let tools = target().join("xtask/card");
    crate::util::fresh_dir(&tools)?;
    let shots = target().join("xtask/card-shots");
    crate::util::fresh_dir(&shots)?;
    let hand = tools.join("card-ci");
    run(Command::new("swiftc").arg("-O").arg("-o").arg(&hand).arg(repo().join("test/fixtures/card-ci.swift")))?;
    // The editor as an app of its own, so LaunchServices opens and activates it as it would the person's.
    let editor_app = tools.join("Editor.app");
    fs::create_dir_all(editor_app.join("Contents/MacOS")).map_err(|e| e.to_string())?;
    run(Command::new("swiftc")
        .arg("-O")
        .arg("-o")
        .arg(editor_app.join("Contents/MacOS/Editor"))
        .arg(repo().join("test/fixtures/fullscreen-editor.swift")))?;
    fs::write(
        editor_app.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleExecutable</key><string>Editor</string><key>CFBundleIdentifier</key><string>dev.sidevoice.ci-editor</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleName</key><string>Editor</string></dict></plist>"#,
    )
    .map_err(|e| e.to_string())?;
    run(Command::new(&hand).arg("trusted"))?;

    let home = scratch_home("card")?;
    let (card_log, editor_log) = (logs("card.log"), logs("editor.log"));
    let app = Background::start(&mut app_command(&home, &[("SIDEVOICE_DEBUG_PAGE", "card-probe.html")]), &card_log)?;
    let mut walk = CardRun {
        hand,
        shots,
        card_log,
        editor_log,
        app,
        editor: None,
        checks: Checks::default(),
        at: Rect::default(),
    };
    let walked = walk.walk();
    walk.stop();
    let card_text = read(&walk.card_log);
    eprintln!("--- app");
    for line in card_text.lines().filter(|l| l.contains("call-controls") || l.contains("probe-card")) {
        eprintln!("{line}");
    }
    eprintln!("--- editor\n{}", read(&walk.editor_log));
    walked?;
    walk.checks.done("call controls card")
}

#[derive(Default, Clone, Copy, PartialEq)]
struct Rect {
    x: i64,
    y: i64,
    w: i64,
    h: i64,
}

struct CardRun {
    hand: PathBuf,
    shots: PathBuf,
    card_log: PathBuf,
    editor_log: PathBuf,
    app: Background,
    editor: Option<u32>,
    checks: Checks,
    /// The card's window as last seen on screen (global points).
    at: Rect,
}

impl CardRun {
    fn hand(&self, args: &[String]) -> Result<String> {
        output(Command::new(&self.hand).args(args))
    }

    fn act(&self, verb: &str, args: &[i64]) -> Result<()> {
        let mut words = vec![verb.to_owned()];
        words.extend(args.iter().map(i64::to_string));
        self.hand(&words).map(|_| ())
    }

    fn shot(&self, name: &str) {
        let _ = run(Command::new("screencapture").arg("-x").arg(self.shots.join(format!("{name}.png"))));
    }

    fn expect(&mut self, log: &Path, pattern: &str, after: usize) {
        let seen = wait_for(log, pattern, after, 60);
        self.checks.check(seen, format!("never saw in {}: {pattern}", log.display()));
    }

    fn card_lines(&self) -> usize {
        line_count(&self.card_log)
    }

    /// The card's window as the window server has it: it must be on screen, in front of the editor.
    fn card(&mut self) -> Result<()> {
        let editor = self.editor.unwrap_or(0).to_string();
        let seen = self.hand(&["windows".into(), self.app.pid().to_string(), editor]).unwrap_or_else(|e| e);
        eprintln!("{seen}");
        self.checks.check(
            seen.contains("card onscreen=true above_editor=true editor_onscreen=true"),
            format!("the card is not on screen in front of the editor: {}", seen.lines().next().unwrap_or("")),
        );
        if let Some(line) = seen.lines().find(|l| l.contains(" x=")) {
            let field = |key: &str| line.split(key).nth(1).and_then(|r| r.split(' ').next()?.parse::<i64>().ok());
            if let (Some(x), Some(y), Some(w), Some(h)) = (field(" x="), field(" y="), field(" w="), field(" h=")) {
                self.at = Rect { x, y, w, h };
            }
        }
        Ok(())
    }

    fn editor_active(&mut self, when: &str) {
        let text = read(&self.editor_log);
        let active = lines_matching(&text, "editor active=", 0).last().is_some_and(|l| l.contains("active=true"));
        self.checks.check(active, format!("the editor is not the active app ({when})"));
    }

    fn near(&self) -> Result<()> {
        self.act("move", &[self.at.x + 160, self.at.y + 30])
    }

    fn walk(&mut self) -> Result<()> {
        let (card_log, editor_log) = (self.card_log.clone(), self.editor_log.clone());
        if !wait_for(&card_log, "probe-card phase=talking", 0, 60) {
            return Err("the probe never started".into());
        }
        // The person's editor, full screen in its own Space: the room's window loses focus and the card shows over
        // it, a non-activating panel at the status level, on every Space and over full-screen apps, never key.
        fs::write(&editor_log, "").map_err(|e| e.to_string())?;
        run(Command::new("open")
            .arg("--stdout")
            .arg(&editor_log)
            .arg("--stderr")
            .arg(&editor_log)
            .arg(self.hand.with_file_name("Editor.app")))?;
        if !wait_for(&editor_log, "editor ready pid=", 0, 60) {
            return Err("the editor never started".into());
        }
        self.editor = read(&editor_log).lines().find_map(|l| {
            l.split("editor ready pid=").nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
        });
        self.expect(&editor_log, "editor fullscreen=true key=true", 0);
        if !wait_for(&card_log, "call-controls shown", 0, 60) {
            return Err("the card never showed".into());
        }
        self.expect(&card_log, "call-controls panel ", 0);
        let text = read(&card_log);
        let panel = lines_matching(&text, "call-controls panel ", 0).last().unwrap_or("").to_owned();
        self.checks.check(
            crate::util::matches(
                &panel,
                "nonactivating=true level=25 all_spaces=true fullscreen_auxiliary=true key=false visible=true .* app_active=false",
            ),
            format!("the panel is not what #4 asks: {panel}"),
        );
        self.expect(&card_log, "call-controls watching clicks elsewhere: true", 0);
        self.expect(&card_log, "call-controls prevents activation: true", 0);
        sleep(3.0);
        self.card()?;
        if self.at == Rect::default() {
            return Err("the card's window was never found".into());
        }
        self.shot("1-over-fullscreen-you-talking");
        // The card's page may not capture: it asked for the microphone (probe build) and was refused.
        self.expect(&card_log, r#"call-controls refused {"capture":".*","command":"probe-capture"}"#, 0);
        let text = read(&card_log);
        let refusal = lines_matching(&text, r#"call-controls refused {"capture""#, 0).last().unwrap_or("").to_owned();
        self.checks.check(
            refusal.contains(r#""capture":"NotAllowedError""#),
            "the card's page was not refused the microphone",
        );
        self.checks
            .check(has(&text, "call-controls permission Microphone denied"), "the app did not deny the card's request");
        // The editor has the keyboard.
        self.hand(&["type".into(), "abc".into()])?;
        self.expect(&editor_log, "editor text=abc", 0);

        // The pointer over the card (the app polls it: a panel that is never key gets no hover): its controls appear
        // below it, and the window grows to hold them.
        self.near()?;
        self.expect(&card_log, "call-controls pointer inside=Some(true) app_active=false", 0);
        sleep(1.5);
        self.card()?;
        self.checks.check(self.at.h >= 110, format!("the card did not grow with its controls (h={})", self.at.h));
        self.shot("2-near-you-talking");

        // A click on the title (no move) opens the conversations below the controls; a click in the editor closes it.
        self.act("click", &[self.at.x + 90, self.at.y + 31])?;
        sleep(1.5);
        self.card()?;
        self.checks.check(self.at.h >= 200, format!("the conversations did not open (h={})", self.at.h));
        self.shot("3-conversations");
        let from = self.card_lines();
        self.act("click", &[200, 700])?;
        self.expect(&card_log, "call-controls outside click", from);
        self.editor_active("after a click in it");

        // A drag from the card (not a button) moves it; let go, it is dropped and remembered. The app stays inactive.
        self.near()?;
        self.expect(&card_log, "call-controls pointer inside=Some(true)", from);
        sleep(1.0);
        self.card()?;
        let before = (self.at.x, self.at.y);
        let from = self.card_lines();
        self.act("drag", &[self.at.x + 40, self.at.y + 39, -300, 200])?;
        self.expect(&card_log, "call-controls dropped at .* app_active=false", from);
        sleep(1.0);
        self.card()?;
        self.checks.check((self.at.x, self.at.y) != before, "the card did not move");
        self.shot("4-dragged");
        self.editor_active("after a drag");

        // Its mute button, clicked: the command reaches the room, the app never becomes active, and typing still
        // reaches the editor. Muted, then unmuted again (the probe then moves on).
        for state in ["muted", "unmuted"] {
            self.near()?;
            sleep(1.0);
            self.card()?;
            let from = self.card_lines();
            self.act("click", &[self.at.x + 66, self.at.y + self.at.h - 37])?;
            self.expect(&card_log, "call-controls run toggle-mute app_active=false", from);
            self.expect(&card_log, "probe-card command toggle-mute", from);
            sleep(1.0);
            self.shot(&format!("5-{state}"));
            self.editor_active(&format!("after mute ({state})"));
        }
        self.hand(&["type".into(), "def".into()])?;
        self.expect(&editor_log, "editor text=abc.*def", 0);
        self.editor_active("at the end of the clicks");

        // The other states, near (controls) and away (at rest).
        self.expect(&card_log, "probe-card phase=working", 0);
        sleep(2.0);
        self.shot("6-near-agent-working");
        self.expect(&card_log, "probe-card phase=speaking", 0);
        sleep(2.0);
        self.shot("7-near-agent-speaking");
        let from = self.card_lines();
        self.act("move", &[200, 700])?;
        self.expect(&card_log, "call-controls pointer inside=Some(false) app_active=false", from);
        self.expect(&card_log, "probe-card phase=muted", 0);
        sleep(2.0);
        self.shot("8-rest-muted-agent-speaking");
        self.expect(&card_log, "probe-card phase=reconnecting", 0);
        sleep(2.0);
        self.shot("9-rest-reconnecting");
        // Its hang-up button ends the call; the card goes with it (after that, not from an earlier hide).
        self.checks.check(!has(&read(&card_log), "call-controls hidden"), "the card hid before the call ended");
        self.card()?;
        self.near()?;
        sleep(1.5);
        self.card()?;
        let from = self.card_lines();
        self.act("click", &[self.at.x + self.at.w - 45, self.at.y + self.at.h - 37])?;
        self.expect(&card_log, "probe-card command hang-up", from);
        let text = read(&card_log);
        let hung_up = text
            .lines()
            .enumerate()
            .filter(|(_, l)| l.contains("probe-card command hang-up"))
            .map(|(i, _)| i + 1)
            .last();
        self.expect(&card_log, "call-controls hidden", hung_up.unwrap_or(from));
        self.expect(&card_log, "probe-card done", from);
        let editor_text = read(&editor_log);
        self.checks.check(!has(&editor_text, "editor active=false"), "the editor stopped being the active app");
        // Nor the keyboard: its window stays key from full screen on (a loss put right later still counts).
        let since_fullscreen = editor_text.lines().skip_while(|l| !l.contains("editor fullscreen=true key=true"));
        if let Some(lost) = since_fullscreen.into_iter().find(|l| l.contains("editor key=false")) {
            self.checks.fail(format!("the editor's window lost the keyboard: {lost}"));
        }
        Ok(())
    }

    fn stop(&mut self) {
        self.app.stop();
        if let Some(editor) = self.editor {
            let _ = Command::new("kill").arg(editor.to_string()).status();
        }
    }
}

/// Selecting models through the vendored room itself (test/fixtures/room-flow.js): consent, cancel, a failed load,
/// a slow one, persistence. From nothing on disk, so the room downloads what it selects.
fn room_flow() -> Result<()> {
    let home = scratch_home("room")?;
    let (app_log, node_log) = (logs("room.log"), logs("room-node.log"));
    // The machine the room is paired with (its identity proven; it stores the choices per machine).
    let mut node = fake_node(&node_log)?;
    // Real models; only a refused load (whisper-tiny) and a slow transcription (whisper-base on Core ML) injected.
    let mut app = Background::start(
        &mut app_command(
            &home,
            &[
                ("SIDEVOICE_DEBUG_ROOM_FLOW", "1"),
                ("SIDEVOICE_DEBUG_REFUSE_LOAD", "whisper-tiny"),
                ("SIDEVOICE_DEBUG_SLOW_TRANSCRIBE", "whisper-base/coreml:2500"),
            ],
        ),
        &app_log,
    )?;
    for _ in 0..1200 {
        let text = read(&app_log);
        if has(&text, "room-flow ok") || has(&text, "room-flow error") {
            break;
        }
        sleep(0.5);
    }
    app.stop();
    node.stop();
    let text = read(&app_log);
    eprintln!("{text}");
    eprintln!("{}", read(&repo().join("ui/voice/web-source.json")));

    let mut c = Checks::default();
    // The actual vendored room (ui/voice), not a test page: its settings actions, controller, selection, native worker
    // and storage, over this app's bridge and engine.
    c.check(has(&text, "page tauri://localhost/voice/index.html"), "the vendored room did not load");
    let line = text.lines().find(|l| l.contains("room-flow ok")).unwrap_or("").to_owned();
    c.check(!line.is_empty(), "the room flow did not pass");
    // What the room asked the app, and the offers it resolved from it: the five native models, nothing of the page's.
    c.check(
        line.contains(r#"capabilities={"runs":"native","os":"macos","arch":"aarch64","has":["cpu","coreml""#),
        "capabilities are not native macOS arm64 with cpu and coreml",
    );
    c.check(at_least(&line, r#""memory_mb":"#, 1000), "no real memory_mb");
    c.check(
        word_after(&line, " engines=").is_some_and(|w| {
            !w.is_empty()
                && w.split(',').filter(|e| !e.is_empty()).all(|e| {
                    e.strip_prefix("sherpa-onnx/")
                        .is_some_and(|a| !a.is_empty() && a.chars().all(|c| c.is_ascii_lowercase()))
                })
        }),
        "the engines are not sherpa-onnx's alone",
    );
    for model in ["whisper-tiny", "whisper-base", "whisper-small", "whisper-large-v3-turbo"] {
        c.check(
            word_after(&line, " offers_stt=").is_some_and(|w| w.contains(model)),
            format!("{model} is not offered"),
        );
    }
    c.check(line.contains(" offers_tts=kokoro-82m-v1.0 "), "Kokoro is not the one TTS offer");
    c.check(!["transformers-js", "webgpu", "wasm"].iter().any(|p| line.contains(p)), "a page engine in the app");
    c.check(word_after(&line, " shown_stt=").is_some_and(|w| w.contains("whisper-tiny")), "whisper-tiny is not shown");
    c.check(word_after(&line, " shown_tts=").is_some_and(|w| w.contains("kokoro-82m-v1.0")), "Kokoro is not shown");
    c.check(line.contains(" where=app "), "the models do not run in the app");
    // 1. Consent with the size, then download → load → two passes → in effect, stored, loaded.
    c.check(
        number_after(&line, " consent_size=").is_some_and(|(size, rest)| {
            size >= 10_000_000
                && rest.starts_with(" stored_before=none chosen=whisper-base/auto chosen_resident=whisper-base@sherpa-onnx/cpu chosen_passes=2 ")
        }),
        "consent, download and selection did not go as the flow says",
    );
    // 2. Cancelled mid-download: nothing stored, nothing loaded, nothing installed, the download row cancelled.
    c.check(
        number_after(&line, " cancel_at=").is_some_and(|(done, rest)| {
            done >= 1
                && rest.strip_prefix('/').and_then(|r| number_after(r, "")).is_some_and(|(total, tail)| {
                    total >= 1
                        && tail.starts_with(" cancel_download=cancelled cancel_stored=whisper-base/auto cancel_resident=whisper-base@sherpa-onnx/cpu cancel_installed=false ")
                })
        }),
        "a cancelled download did not leave things as they were",
    );
    c.check(!cancelled_left_files(&home), "a cancelled download left files");
    // 3. A failed load: its step and cause, the model in use still in use and alone in memory.
    c.check(
        ["runtime_failed", "load_failed"].iter().any(|key| {
            line.contains(&format!(
                " fail_step=load fail_key={key} fail_stored=whisper-base/auto fail_resident=whisper-base@sherpa-onnx/cpu "
            ))
        }),
        "a failed load did not keep the model in use",
    );
    // 4. Slow on Core ML: declined, nothing changes and its copy goes; accepted, it is stored and the old copy goes.
    c.check(
        number_after(&line, " slow_latency_ms=").is_some_and(|(ms, rest)| {
            (2000..=9999).contains(&ms)
                && rest
                    .starts_with(" declined_stored=whisper-base/auto declined_resident=whisper-base@sherpa-onnx/cpu ")
        }),
        "a slow model, declined, changed something",
    );
    for fact in [
        " accepted_stored=whisper-base/coreml accepted_resident=whisper-base@sherpa-onnx/coreml ",
        // 5. After a reload, with the machine reached: settings show the stored choice; the next selection starts
        // from it.
        " reload_reach=ok reload_restored=whisper-base/coreml reload_restored_pane=whisper-base/sherpa-onnx/coreml ",
        " reload_after_stored=whisper-base/auto reload_after_resident=whisper-base@sherpa-onnx/cpu",
    ] {
        c.check(line.contains(fact), format!("the room flow lacks{fact}"));
    }
    c.check(has(&read(&node_log), "request GET /api/device/identity"), "the room never asked the machine's identity");
    c.done("room flow")
}
