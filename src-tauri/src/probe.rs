// Existing OS debt is capped by platform-debt.txt.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

//! CI only, compiled only with the `probe` feature (never in a release):
//! - the probe page (`test/fixtures/probe.html`), served at the app's own origin and loaded into the room window
//!   instead of the interface when `SIDEVOICE_DEBUG=1 SIDEVOICE_DEBUG_PAGE=probe.html`: what the interface's window
//!   offers with that window's own origin and permission rule;
//! - the call controls probe page (`test/fixtures/card-probe.html`), loaded the same way with
//!   `SIDEVOICE_DEBUG_PAGE=card-probe.html`: a call walked through the states the card shows, for CI's screenshots;
//!   with it, the card's own page asks for the microphone once ([`card_script`]), which the app must refuse;
//! - the local-host flow (`test/fixtures/local-host-flow.js`), injected into the room window with
//!   `SIDEVOICE_DEBUG_LOCAL_HOST_FLOW=1`: the local host and its proxy as the bundled room's page reaches them;
//!
//! All run in CI's macOS job (.github/workflows/build.yml). A release app has none of them, nor the switches; CI
//! checks.

use std::borrow::Cow;
use tauri::utils::assets::{AssetKey, AssetsIter, CspHash};
use tauri::{App, AppHandle, Assets, Context, Runtime};

/// With `SIDEVOICE_DEBUG_EXIT=quit` or `=restart`, once the probe page logs its last line (`probe-exit`, a model on the
/// GPU loaded): the app quits as the tray's Quit does, or restarts as an installed update does (`AppHandle::restart`
/// from a thread of its own, src/updater.rs). The relaunched app inherits this environment, told `quit`: it quits after
/// its own run. `macos exit` checks that both end cleanly (#121).
pub fn exit_on(app: &AppHandle, line: &str) {
    if !crate::debugging() || !line.starts_with("probe-exit ") {
        return;
    }
    match std::env::var("SIDEVOICE_DEBUG_EXIT").as_deref() {
        Ok("quit") => {
            crate::debug("probe: quitting");
            app.exit(0);
        }
        Ok("restart") => {
            crate::debug("probe: restarting");
            std::env::set_var("SIDEVOICE_DEBUG_EXIT", "quit");
            let app = app.clone();
            std::thread::spawn(move || app.restart());
        }
        _ => {}
    }
}

/// With `SIDEVOICE_DEBUG_EXIT`, from the start: a handler `exit()` runs after the static destructors of the C++
/// libraries linked in. `exit()` runs the handlers registered with `atexit` and those destructors (`__cxa_atexit`,
/// when each static is built) from one list, the last registered first; this one is registered before any model is
/// loaded, so before whisper.cpp's Metal device is built, and runs after its destructor, which aborts when a model is
/// still loaded (#121). Its line in the log says that destructor ran to its end. `macos exit` checks it.
#[cfg(target_os = "macos")]
pub fn watch_exit_handlers() {
    if !crate::debugging() || std::env::var_os("SIDEVOICE_DEBUG_EXIT").is_none() {
        return;
    }
    extern "C" fn ran() {
        eprintln!("sidevoice: probe: exit handlers ran");
    }
    // SAFETY: registers a plain function, which only writes to stderr.
    unsafe { libc::atexit(ran) };
}

/// With `SIDEVOICE_DEBUG_PAGE=memory.html`, at each line `probe-memory PHASE` the page logs: the app's memory then,
/// printed as `memory phase=PHASE ...` (core `memory`), on a thread of its own (`vmmap` takes a second or two; the page
/// waits for it). `macos memory` reads the lines back (#145).
pub fn memory_on(app: &tauri::AppHandle, line: &str) {
    let Some(phase) = line.strip_prefix("probe-memory ") else { return };
    if !crate::debugging() {
        return;
    }
    let phase = phase.trim().to_owned();
    let _ = app;
    #[cfg(target_os = "macos")]
    if let Some(state) = tauri::Manager::try_state::<crate::voice::VoiceState>(app) {
        println!("memory-holds phase={phase} {}", state.held());
    }
    std::thread::spawn(move || {
        crate::memory::report(&phase);
    });
}

/// With `SIDEVOICE_DEBUG_IDLE_MINUTE_MS`, how long the voice host's idle minute is (src/voice/host.rs): the models of a
/// stopped call stay loaded for `idle_unload_minutes` of them, so `macos memory` can wait for them to be let go.
#[cfg(target_os = "macos")]
pub fn idle_minute() -> std::time::Duration {
    let millis = std::env::var("SIDEVOICE_DEBUG_IDLE_MINUTE_MS").ok().and_then(|v| v.parse::<u64>().ok());
    match millis {
        Some(millis) if crate::debugging() && millis > 0 => std::time::Duration::from_millis(millis),
        _ => std::time::Duration::from_secs(60),
    }
}

pub const PAGE: &str = "probe.html";
pub const MEMORY_PAGE: &str = "memory.html";
const MEMORY_HTML: &[u8] = include_bytes!("../../test/fixtures/memory.html");
const HTML: &[u8] = include_bytes!("../../test/fixtures/probe.html");
pub const CARD_PAGE: &str = "card-probe.html";
const CARD_HTML: &[u8] = include_bytes!("../../test/fixtures/card-probe.html");
const LOCAL_HOST_FLOW: &str = include_str!("../../test/fixtures/local-host-flow.js");

/// The page the room window loads: a probe when asked for, else `interface`.
pub fn page(interface: &str) -> String {
    match std::env::var("SIDEVOICE_DEBUG_PAGE") {
        Ok(asked) if crate::debugging() && (asked == PAGE || asked == CARD_PAGE || asked == MEMORY_PAGE) => asked,
        _ => interface.to_string(),
    }
}

/// With the call controls probe: a script for the card's own page that asks for the microphone, as script that got
/// into the card could, and sends back what it got as a command the app refuses and logs ("call-controls refused").
/// It also says, the same way, each time the pointer comes over a button (or within one) and its CSS cursor, as WebKit sees them (#139): only
/// a page that gets the pointer's moves sees `:hover` change before a click.
pub fn card_script() -> Option<&'static str> {
    let asked = std::env::var("SIDEVOICE_DEBUG_PAGE").is_ok_and(|v| v == CARD_PAGE);
    (crate::debugging() && asked).then_some(
        r#"(() => {
  // Once the card shows (WebKit may hold a hidden page's request back), or after 20 s whatever it is.
  let asked = false;
  const ask = (now) => {
    if (asked || !(now || document.visibilityState === "visible")) return;
    asked = true;
    const report = (capture) =>
      window.__TAURI_INTERNALS__.invoke("call_controls_run", { command: { command: "probe-capture", capture } });
    if (!navigator.mediaDevices) return report("no-media-devices");
    navigator.mediaDevices.getUserMedia({ audio: true }).then(
      (stream) => { stream.getTracks().forEach((t) => t.stop()); report("granted"); },
      (error) => report(String(error && error.name)));
  };
  document.addEventListener("visibilitychange", () => ask(false));
  setTimeout(() => ask(true), 20000);
  document.addEventListener("mouseover", () => {
    const all = document.querySelectorAll(":hover");
    const deepest = all[all.length - 1];
    const button = deepest && deepest.closest("button");
    if (!button) return;
    window.__TAURI_INTERNALS__.invoke("call_controls_run",
      { command: { command: "probe-hover", button: String(button.className), cursor: getComputedStyle(deepest).cursor } });
  }, true);
})();"#,
    )
}

/// The script that walks the local host through the bundled room's own page (`test/fixtures/local-host-flow.js`),
/// when asked for with `SIDEVOICE_DEBUG_LOCAL_HOST_FLOW=1`; injected into the room window.
pub fn local_host_flow() -> Option<&'static str> {
    let asked = std::env::var("SIDEVOICE_DEBUG_LOCAL_HOST_FLOW").is_ok_and(|v| v == "1");
    (crate::debugging() && asked).then_some(LOCAL_HOST_FLOW)
}

/// The app's own assets, plus the probe page.
pub fn with_page<R: Runtime>(mut context: Context<R>) -> Context<R> {
    let bundled = context.set_assets(Box::new(Nothing));
    context.set_assets(Box::new(WithProbe(bundled)));
    context
}

struct Nothing;
struct WithProbe<R: Runtime>(Box<dyn Assets<R>>);

impl<R: Runtime> Assets<R> for Nothing {
    fn get(&self, _key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        None
    }
    fn iter(&self) -> Box<AssetsIter<'_>> {
        Box::new(std::iter::empty())
    }
    fn csp_hashes(&self, _html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        Box::new(std::iter::empty())
    }
}

impl<R: Runtime> Assets<R> for WithProbe<R> {
    fn setup(&self, app: &App<R>) {
        self.0.setup(app)
    }
    fn get(&self, key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        match key.as_ref().trim_start_matches('/') {
            PAGE => return Some(Cow::Borrowed(HTML)),
            CARD_PAGE => return Some(Cow::Borrowed(CARD_HTML)),
            MEMORY_PAGE => return Some(Cow::Borrowed(MEMORY_HTML)),
            _ => {}
        }
        self.0.get(key)
    }
    fn iter(&self) -> Box<AssetsIter<'_>> {
        self.0.iter()
    }
    fn csp_hashes(&self, html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        self.0.csp_hashes(html_path)
    }
}
