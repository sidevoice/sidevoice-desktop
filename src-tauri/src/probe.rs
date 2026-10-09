//! CI only, compiled only with the `probe` feature (never in a release):
//! - the probe page (`test/fixtures/probe.html`), served at the app's own origin and loaded into the room window
//!   instead of the interface when `SIDEVOICE_DEBUG=1 SIDEVOICE_DEBUG_PAGE=probe.html`: what the interface's window
//!   offers with that window's own origin and permission rule;
//! - the call controls probe page (`test/fixtures/card-probe.html`), loaded the same way with
//!   `SIDEVOICE_DEBUG_PAGE=card-probe.html`: a call walked through the states the card shows, for CI's screenshots;
//!   with it, the card's own page asks for the microphone once ([`card_script`]), which the app must refuse;
//! - the local-host flow (`test/fixtures/local-host-flow.js`), injected into the room window with
//!   `SIDEVOICE_DEBUG_LOCAL_HOST_FLOW=1`: the local host and its proxy as the vendored room's page reaches them;
//!
//! All run in CI's macOS job (.github/workflows/build.yml). A release app has none of them, nor the switches; CI
//! checks.

use std::borrow::Cow;
use tauri::utils::assets::{AssetKey, AssetsIter, CspHash};
use tauri::{App, Assets, Context, Runtime};

pub const PAGE: &str = "probe.html";
const HTML: &[u8] = include_bytes!("../../test/fixtures/probe.html");
pub const CARD_PAGE: &str = "card-probe.html";
const CARD_HTML: &[u8] = include_bytes!("../../test/fixtures/card-probe.html");
const LOCAL_HOST_FLOW: &str = include_str!("../../test/fixtures/local-host-flow.js");

/// The page the room window loads: a probe when asked for, else `interface`.
pub fn page(interface: &str) -> String {
    match std::env::var("SIDEVOICE_DEBUG_PAGE") {
        Ok(asked) if crate::debugging() && (asked == PAGE || asked == CARD_PAGE) => asked,
        _ => interface.to_string(),
    }
}

/// With the call controls probe: a script for the card's own page that asks for the microphone, as script that got
/// into the card could, and sends back what it got as a command the app refuses and logs ("call-controls refused").
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
})();"#,
    )
}

/// The script that walks the local host through the vendored room's own page (`test/fixtures/local-host-flow.js`),
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
