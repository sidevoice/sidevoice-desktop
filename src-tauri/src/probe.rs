//! CI only, compiled only with the `probe` feature (never in a release):
//! - the probe page (`test/fixtures/probe.html`), served at the app's own origin and loaded into the room window
//!   instead of the interface when `SIDEVOICE_DEBUG=1 SIDEVOICE_DEBUG_PAGE=probe.html`: what the interface's window
//!   offers with that window's own origin and permission rule;
//! - the room-flow script (`test/fixtures/room-flow.js`), injected into the room window with
//!   `SIDEVOICE_DEBUG_ROOM_FLOW=1`: the native engine driven through the vendored room itself;
//! - `SIDEVOICE_DEBUG_IDLE_UNLOAD_SECS`: how long a model stays in memory unused (D13), shortened so the probe page
//!   sees the app unload a model and load it again as a call connects.
//!
//! All run in CI's macOS job (.github/workflows/build.yml). A release app has none of them, nor the switches; CI
//! checks.

use std::borrow::Cow;
use tauri::utils::assets::{AssetKey, AssetsIter, CspHash};
use tauri::{App, Assets, Context, Runtime};

pub const PAGE: &str = "probe.html";
const HTML: &[u8] = include_bytes!("../../test/fixtures/probe.html");
const ROOM_FLOW: &str = include_str!("../../test/fixtures/room-flow.js");

/// The page the room window loads: the probe when asked for, else `interface`.
pub fn page(interface: &str) -> String {
    match std::env::var("SIDEVOICE_DEBUG_PAGE") {
        Ok(asked) if crate::debugging() && asked == PAGE => PAGE.to_string(),
        _ => interface.to_string(),
    }
}

/// The script that drives the native flow through the vendored room itself (`test/fixtures/room-flow.js`), when
/// asked for with `SIDEVOICE_DEBUG_ROOM_FLOW=1`; injected into the room window, where the interface loads.
pub fn room_flow() -> Option<&'static str> {
    let asked = std::env::var("SIDEVOICE_DEBUG_ROOM_FLOW").is_ok_and(|v| v == "1");
    (crate::debugging() && asked).then_some(ROOM_FLOW)
}

/// How long a model stays in memory unused (D13), shortened with `SIDEVOICE_DEBUG_IDLE_UNLOAD_SECS` so the probe
/// can watch the app unload it and load it again as a call connects. Ten minutes otherwise, as in a release.
pub fn idle_unload() -> Option<std::time::Duration> {
    let seconds = std::env::var("SIDEVOICE_DEBUG_IDLE_UNLOAD_SECS").ok()?.parse().ok()?;
    crate::debugging().then(|| std::time::Duration::from_secs(seconds))
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
        if key.as_ref().trim_start_matches('/') == PAGE {
            return Some(Cow::Borrowed(HTML));
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
