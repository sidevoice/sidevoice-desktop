#![cfg(mobile)]
//! The Sidevoice mobile app: a native shell around the Sidevoice web interface the repository pins (`web.pin.json`,
//! unpacked into `ui/` by `npm run web`, which every Tauri build and dev run starts with).
//!
//! The window shows only the app's own pages, and gives the page the app's members (`window.__sidevoiceHost`,
//! src/bridge.rs): the voice call the app runs, its engine, the microphone's permission and the pairings, through the
//! commands every app shares (`sidevoice-shared-tauri`, over `sidevoice-shared`'s application host). The page reports
//! the call back (`call_state`), which the headset's buttons and the lock screen's controls follow. No page captures
//! anything: the app owns the microphone and the speaker, as on the desktop (docs/MOBILE.md).
#[cfg(test)]
mod acl_tests;
mod bridge;
use bridge::headset;
mod call;
mod developer_options;
mod downloads;
mod hold;
mod i18n;
pub mod pairings;
mod panics;
mod platform;
mod surface;
mod system_log;

use std::{path::PathBuf, sync::Arc};

pub use platform::entry::run;
use sidevoice_mobile_plugin::SurfaceAction;
use sidevoice_shared::app::{AlwaysAdmit, AppHost, Hooks, HostConfig, Keychain};
use sidevoice_shared_tauri::platform::media_controls::{self, CallState, MediaCommand};
use sidevoice_shared_tauri::platform::origin::APP_ORIGIN;
use sidevoice_shared_tauri::{Pages, Pairings};
use tauri::{
    webview::{PageLoadEvent, PermissionResponse},
    AppHandle, Manager, Runtime, Url, Webview, WebviewUrl, WebviewWindowBuilder,
};

/// The app's one window: the room page, the only caller of the shared commands (capabilities/main.json).
const WINDOW: &str = "main";

/// The namespace of this app's provider keys in the OS credential store (docs/CREDENTIALS.md).
const PROVIDER_KEYS: &str = "ai.sidevoice.mobile.providers";

/// The folder of the engine's models in the app's own files: the app's alone (a phone has one app of ours), kept out of
/// the device backup (they are downloaded again).
const MODELS: &str = "models";

/// How long quitting waits, in all, for the call's surface to end, the calls to stop and the models to be released. iOS gives
/// `applicationWillTerminate` about 5 s before the process is killed, so the whole exit path stays under it.
const RELEASE_LIMIT: std::time::Duration = std::time::Duration::from_secs(4);

const BUNDLED_INTERFACE: &str = "voice/index.html";

/// An URL's origin as a page on it reports it. The WHATWG origin of a custom scheme (`tauri://localhost`) is opaque
/// (`null`), but the webviews report the app's pages as `scheme://host`, and so does this.
fn origin_of(url: &Url) -> String {
    match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{}://{host}:{port}", url.scheme()),
        (Some(host), None) => format!("{}://{host}", url.scheme()),
        _ => "null".to_owned(),
    }
}

/// Which navigations the window allows: the app's own pages, and the schemes a page's own frames and objects use.
/// Never another site, a local file or another app scheme.
fn navigation_allowed(url: &Url, app_origin: &str) -> bool {
    origin_of(url) == app_origin || matches!(url.scheme(), "about" | "blob" | "data")
}

/// The app's own files, inside the directory Tauri calls the app data directory (`platform::app_files`): Application
/// Support's app folder on iOS, `filesDir` on Android.
pub(crate) fn own_files<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<PathBuf> {
    let mut files = app.path().app_data_dir()?;
    if let Some(own) = platform::app_files::FILES {
        files.push(own);
    }
    Ok(files)
}

/// Whether a webview of `label` showing a page of `origin` is the page the app owns the calls for: its one window, on the
/// app's own origin. Every shared command asks it first (sidevoice-shared-tauri's ACL.md).
fn is_the_page(label: &str, origin: Option<&str>) -> Result<(), String> {
    if label != WINDOW {
        return Err("not the app's window".into());
    }
    if origin != Some(APP_ORIGIN) {
        return Err("not the app's own page".into());
    }
    Ok(())
}

/// [`is_the_page`] for the plugin.
fn the_page<R: Runtime>(_app: &AppHandle<R>, webview: &Webview<R>) -> Result<(), String> {
    let origin = webview.url().ok().map(|url| origin_of(&url));
    is_the_page(webview.label(), origin.as_deref())
}

/// The folder of the engine's models, made private to the app and kept out of the device backup.
fn model_root<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let models = own_files(app)?.join(MODELS);
    platform::private_dir::create_private_dir(&models)?;
    platform::backup::exclude_from_backup(&models)?;
    Ok(models)
}

/// The call as the page reports it (`window.__sidevoiceHost`'s room, mobile/tauri/js/mobile-bridge.js): what the app reads of
/// it. The rest of the snapshot is the page's and is not read.
#[derive(Debug, Default, Clone, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct CallReport {
    ready: bool,
    joined: bool,
    mic_enabled: bool,
    /// The mute cannot be used (no conversation is selected).
    mic_disabled: bool,
    title: String,
    /// When the call was joined, as the page marks it (ms since the epoch): a later join is another call.
    since: Option<f64>,
    /// The room's own words for the call, which the call's surfaces read (`surface.rs`): the agent's mood, the person's
    /// open turn, why the call is busy (strings, read leniently: anything else is none), and whether a reply can be
    /// skipped.
    mood: serde_json::Value,
    turn: serde_json::Value,
    busy_reason: serde_json::Value,
    can_skip: bool,
    /// The page's language (`callCard.language`), for what the system words on the call's surfaces.
    language: serde_json::Value,
    /// The page's `callSurface`, as the bridge relays it (its stills by id; their bytes come through `call_still`). Read
    /// apart ([`surface::SurfaceReport::read`]), so that a surface the app cannot read never stops the call.
    surface: serde_json::Value,
}

impl CallReport {
    /// What the call's surfaces are shown of this report.
    fn surface(&self) -> surface::Reported {
        let text = |value: &serde_json::Value| value.as_str().map(str::to_owned);
        surface::Reported {
            mood: text(&self.mood),
            turn: text(&self.turn),
            busy_reason: text(&self.busy_reason),
            can_skip: self.can_skip,
            language: text(&self.language).filter(|language| !language.is_empty()),
            surface: surface::SurfaceReport::read(&self.surface),
        }
    }
}

/// The last report, to log what changed.
#[derive(Default)]
struct LastReport(std::sync::Mutex<CallReport>);

/// `host.debug`'s one command (docs/BRIDGE.md → "The developer options"): `action` `get`, or `set` with a switch's `name` and `enabled`,
/// answering the switches' state. Refused (`bad_request`) in a release, for a switch the phone lacks or another action.
#[tauri::command]
fn developer_options(
    app: AppHandle,
    webview: Webview,
    options: tauri::State<'_, developer_options::DeveloperOptions>,
    action: String,
    name: Option<String>,
    enabled: Option<bool>,
) -> Result<serde_json::Value, serde_json::Value> {
    let refusal = |why: String| serde_json::json!({ "key": "bad_request", "message": why });
    the_page(&app, &webview).map_err(refusal)?;
    match (action.as_str(), name, enabled) {
        ("get", None, None) => options
            .state()
            .ok_or_else(|| refusal("The developer options are not in this build.".into())),
        ("set", Some(name), Some(on)) => options.set(&name, on).map_err(refusal),
        _ => Err(refusal(
            "developer_options takes get, or set with a name and enabled".into(),
        )),
    }
}

/// `host.shortcuts`' one command (docs/BRIDGE.md → "The shortcuts"), the phone's part of it: `action` `get`, or `headset` with the
/// change (`setHeadset`), answering the status, which holds the headset group alone (`headset::HeadsetSettings::status`).
#[tauri::command]
fn global_shortcuts(
    app: AppHandle,
    webview: Webview,
    settings: tauri::State<'_, headset::HeadsetSettings>,
    action: String,
    headset: Option<serde_json::Value>,
) -> Result<serde_json::Value, serde_json::Value> {
    let refusal = |why: String| serde_json::json!({ "key": "bad_request", "message": why });
    the_page(&app, &webview).map_err(refusal)?;
    let system = headset::system_call(&app);
    match (action.as_str(), headset.as_ref()) {
        ("get", None) => Ok(settings.status(system)),
        ("headset", Some(change)) => {
            let status = settings.set(Some(change), system).map_err(refusal)?;
            // The media controls follow whether the double press has an action (Android's skip-to-next), also within a call.
            if let Some(room) = app.try_state::<call::RoomCall>() {
                room.headset_changed();
            }
            Ok(status)
        }
        _ => Err(refusal(
            "global_shortcuts takes get, or headset with the change".into(),
        )),
    }
}

/// `host.cue`'s one command (docs/BRIDGE.md -> "The cues"): `action` `available` (whether this phone plays the page's cues
/// itself), or `start` and `stop` with the cue's `name`; a start may say which `sound` (an id of the bundled web's manifest,
/// else its default), its `gainDb` (the person's offset, else 0), its `intervalScale` (the person's frequency: what multiplies
/// the silence after each play, 0.5 to 3; anything else, or none, 1) and `whileLocked` (true: it plays only while the screen is
/// locked, from each lock to the unlock, `When::WhileLocked`; else at once). The sound is read from the app's own assets, the
/// web it bundles. The player belongs to the main thread.
#[tauri::command]
// The page's own arguments (six) and the app's two: a command is its wire.
#[allow(clippy::too_many_arguments)]
fn cue(
    app: AppHandle,
    webview: Webview,
    action: String,
    name: Option<String>,
    sound: Option<String>,
    gain_db: Option<f64>,
    interval_scale: Option<f64>,
    while_locked: Option<bool>,
) -> Result<bool, serde_json::Value> {
    use sidevoice_mobile_plugin::{
        cue_available, cue_current, cue_ticket, end_cues, gain_db as offset,
        interval_scale as frequency, prepare_cue, start_cue, stop_cue, Cue, When,
    };
    // Every refusal goes to the app's log at warn: the page says it only in its own console.
    let refusal = |why: &str| {
        log::warn!(target: "cue", "cue refused: {why}");
        serde_json::json!({ "key": "bad_request", "message": why })
    };
    the_page(&app, &webview).map_err(|why| refusal(&why))?;
    if action == "available" {
        return Ok(cue_available(|path| assets(&app, path)));
    }
    let cue = name
        .as_deref()
        .and_then(Cue::named)
        .ok_or_else(|| refusal("cue takes the name of a cue the app knows"))?;
    let start = match action.as_str() {
        "start" => true,
        "stop" => false,
        _ => return Err(refusal("cue takes available, start or stop")),
    };
    if !start {
        // A start still being read when the page stops the cue plays nothing.
        end_cues();
        app.run_on_main_thread(move || stop_cue(cue))
            .map_err(|_| refusal("the main thread did not take it"))?;
        return Ok(true);
    }
    let gain_db = offset(gain_db).map_err(|why| refusal(&why))?;
    let interval_scale = frequency(interval_scale);
    let when = When::asked(while_locked);
    // Only in a call whose audio session is open: a page outside a call plays nothing and touches no session. Asked in the room's
    // own order, so that a start after the call's end finds it ended (and the call's end stops what played: `Room::stop_cue`).
    let room = app
        .try_state::<call::RoomCall>()
        .ok_or_else(|| refusal("no call"))?;
    let handle = app.clone();
    // Bound to the request: a later start, a stop, a call's end or a page's teardown before it plays makes it a no-op, in
    // `start_cue`.
    let ticket = cue_ticket();
    room.then(move |room| {
        if !cue_current(ticket) {
            log::info!(target: "cue", "cue {cue:?}: a start superseded before it was checked");
            return;
        }
        if let Some(why) = room.cue_refusal() {
            log::warn!(target: "cue", "cue {cue:?} refused: {why}");
            return;
        }
        // Read and decoded on a thread of its own: neither the main thread nor the room's, whose next job may be the call's end.
        // A start made stale meanwhile (a stop, another start, the call's end) is a no-op in `start_cue`; an end queued behind it
        // on the main thread stops what it played.
        let spawned = std::thread::Builder::new()
            .name("cue".into())
            .spawn(move || {
                let seed = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(1, |since| since.subsec_nanos());
                let prepared = match prepare_cue(
                    |path| assets(&handle, path),
                    sound.as_deref(),
                    gain_db,
                    interval_scale,
                    seed,
                ) {
                    Ok(prepared) => prepared,
                    Err(_) => {
                        log::warn!(target: "cue", "cue {cue:?} did not start: the sound was not prepared");
                        return;
                    }
                };
                if !cue_current(ticket) {
                    log::info!(target: "cue", "cue {cue:?}: a start superseded while its sound was read");
                    return;
                }
                let queued = handle.run_on_main_thread(move || {
                    if start_cue(cue, prepared, ticket, when).is_err() {
                        log::warn!(target: "cue", "cue {cue:?} did not start");
                    }
                });
                if queued.is_err() {
                    log::warn!(target: "cue", "cue {cue:?} did not start: the main thread did not take it");
                }
            });
        if let Err(error) = spawned {
            log::warn!(target: "cue", "cue {cue:?} did not start: no thread (os error {:?})", error.raw_os_error());
        }
    });
    Ok(true)
}

/// `work` on the main thread with the platform's web view, and its answer: UIKit's objects are the main thread's.
async fn on_main<T: Send + 'static>(
    webview: &Webview,
    work: impl FnOnce(&tauri::webview::PlatformWebview) -> T + Send + 'static,
) -> Result<T, String> {
    let (done, answer) = std::sync::mpsc::channel();
    webview
        .with_webview(move |platform| {
            let _ = done.send(work(&platform));
        })
        .map_err(|error| error.to_string())?;
    // The main thread answers; waiting is on a blocking thread, never on the main one.
    tauri::async_runtime::spawn_blocking(move || answer.recv())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

/// `host.audioRoute` (docs/BRIDGE.md -> "The audio route"): `action` `available` (whether this phone has a system picker for where
/// the call's audio plays, which the app can open), `current` (the name of the output it plays through now, or null) or `open` (opens it over the app's window; the person's choice is the system's,
/// and the page is told of nothing). The picker's view belongs to the main thread, where `with_webview` runs it.
#[tauri::command]
async fn audio_route(
    app: AppHandle,
    webview: Webview,
    action: String,
) -> Result<serde_json::Value, serde_json::Value> {
    let refusal = |why: &str| serde_json::json!({ "key": "bad_request", "message": why });
    the_page(&app, &webview).map_err(|why| refusal(&why))?;
    match action.as_str() {
        "available" => Ok(platform::audio_route_picker::available().into()),
        "current" => Ok(platform::audio_route_picker::current().into()),
        "open" => {
            use platform::audio_route_picker::open;
            on_main(&webview, open)
                .await
                .map_err(|why| refusal(&why))?
                .map_err(|code| serde_json::json!({ "key": "audio_route", "code": code, "message": code }))?;
            Ok(true.into())
        }
        _ => Err(refusal("audio_route takes available, current or open")),
    }
}

/// `host.screen.locked()` (docs/BRIDGE.md -> "The screen"), the desktop's contract: whether the person's screen is locked now, a
/// screen that is off counting as locked (iOS: its protected data gone, which needs a passcode and comes some seconds after the
/// lock; Android: the keyguard, or the screen off), for the page's "only with the screen off" thinking sound: the app in the
/// background with the screen on is not it. Asked from a blocking thread: the system answers on the main thread, which then
/// waits on nothing.
#[tauri::command]
async fn screen_locked(app: AppHandle, webview: Webview) -> Result<bool, serde_json::Value> {
    the_page(&app, &webview)
        .map_err(|why| serde_json::json!({ "key": "bad_request", "message": why }))?;
    tauri::async_runtime::spawn_blocking(move || sidevoice_mobile_plugin::screen_locked(&app))
        .await
        .map_err(|error| serde_json::json!({ "key": "bad_request", "message": error.to_string() }))
}

/// A file of the app's own assets (its `frontendDist`, the web it bundles), by its path there (`voice/...`).
fn assets<R: Runtime>(app: &AppHandle<R>, path: &str) -> Option<Vec<u8>> {
    app.asset_resolver()
        .get(path.to_string())
        .map(|asset| asset.bytes)
}

/// The page's number (the host's page generation), asked once by the page's script when it loads (the app's own
/// command): it sends it back with every report, so that a report of a page that has since been replaced, reaching the
/// app late, is told from the current page's.
#[tauri::command]
fn call_page(app: AppHandle, webview: Webview) -> Result<u64, String> {
    the_page(&app, &webview)?;
    Ok(app.state::<AppHost>().page().0)
}

/// The page `page` reports the call (the app's own command, capabilities/main.json). Logged when it joins or leaves a
/// call, or mutes; never the conversation's title.
#[tauri::command]
fn call_state(
    app: AppHandle,
    webview: Webview,
    last: tauri::State<'_, LastReport>,
    page: u64,
    snapshot: CallReport,
) -> Result<(), String> {
    the_page(&app, &webview)?;
    if page != app.state::<AppHost>().page().0 {
        return Ok(()); // a page that has been replaced: its call is over
    }
    // The green and the red button: the system's call follows the room's call, and the headset's buttons and the lock
    // screen's controls follow the call (`call::RoomCall` checks the page again, in order with the page's changes).
    app.state::<call::RoomCall>().report(
        page,
        snapshot.clone().into(),
        snapshot.since,
        snapshot.mic_disabled,
        snapshot.surface(),
    );
    let previous = std::mem::replace(&mut *last.0.lock().unwrap(), snapshot.clone());
    if (previous.ready, previous.joined, previous.mic_enabled)
        != (snapshot.ready, snapshot.joined, snapshot.mic_enabled)
    {
        log::info!(
            "call_state: ready={} joined={} mic={}",
            snapshot.ready,
            snapshot.joined,
            snapshot.mic_enabled
        );
    }
    Ok(())
}

/// A creature's still for the call's surfaces, from the page `page` (the app's own command, capabilities/main.json;
/// `host.callSurface.still`): a PNG of `px` × `px` in base64, kept by its `id` where the surfaces read it, on the room's
/// thread (`call::RoomCall::save_still`). It answers once the file is there: the page sends the report that names it only
/// after that.
#[tauri::command]
async fn call_still(
    app: AppHandle,
    webview: Webview,
    page: u64,
    id: String,
    png: String,
    px: u32,
) -> Result<(), String> {
    use base64::Engine as _;
    if let Err(error) = the_page(&app, &webview) {
        log::warn!(target: "stills", "call_still from a webview that is not the page: {error}");
        return Err(error);
    }
    log::info!(
        target: "stills",
        "call_still: id of {} bytes ({} base64 bytes, {px} px) from page {page}",
        id.len(),
        png.len()
    );
    // Every refusal is said, in the stills' public category: an id and a size, never a conversation's words.
    let refused = |why: &str| {
        log::warn!(target: "stills", "call_still: id of {} bytes refused: {why}", id.len());
        format!("call_still: {why}")
    };
    if png.len() > sidevoice_mobile_plugin::STILL_LIMIT / 3 * 4 + 4 {
        return Err(refused("larger than a still"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png.as_bytes())
        .map_err(|_| refused("not base64"))?;
    tauri::async_runtime::spawn_blocking(move || {
        let room = app.state::<call::RoomCall>();
        room.save_still(page, id, bytes, px)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| {
        log::warn!(target: "stills", "call_still: not kept");
        format!("call_still: {error}")
    })
}

/// The stills `ids` the page `page` shows no more (the app's own command; `host.callSurface.forget`): their files go
/// once no surface draws them (`call::RoomCall::forget_stills`).
#[tauri::command]
fn call_forget(
    app: AppHandle,
    webview: Webview,
    page: u64,
    ids: Vec<String>,
) -> Result<(), String> {
    the_page(&app, &webview)?;
    app.state::<call::RoomCall>().forget_stills(page, ids);
    Ok(())
}

impl From<CallReport> for CallState {
    fn from(report: CallReport) -> CallState {
        CallState {
            ready: report.ready,
            joined: report.joined,
            mic_enabled: report.mic_enabled,
            title: report.title,
            double_press_wanted: false,
            // iOS: the single press is the system's mute, whatever was chosen (`media_controls::presses`); Android does not read it.
            press_keeps_mic: false,
            // Said by the room's call, which knows whether its audio is open (`call.rs`, `media`).
            record_active: false,
        }
    }
}

/// The app: one window, on the bundled interface, with the members the apps share.
fn run_app() {
    // Before anything that may panic: every panic into a file that the next launch reads, since one the app cannot unwind
    // from aborts it before anything else is written (`panics`).
    panics::install();
    init_logging();
    #[allow(
        clippy::disallowed_macros,
        reason = "the context Tauri generates prints its own diagnostics"
    )]
    let context = tauri::generate_context!();
    // Where the call's events go: over the channel the page opens (the shared plugin's `voice_events`).
    let pages = Pages::default();
    let events = pages.clone();
    tauri::Builder::default()
        // The page's `log` command only (its lines have target `webview`): the plugin installs no logger, and its records reach
        // the subscriber through `log`'s bridge, so one filter and one writer serve every line.
        .plugin(tauri_plugin_log::Builder::new().skip_logger().build())
        .plugin(sidevoice_mobile_plugin::init())
        // The camera that scans the pairing code (`host.codeScanner`): the page asks for its permission and a scan through
        // the plugin's own commands, which the window is granted one by one (capabilities/main.json).
        .plugin(tauri_plugin_barcode_scanner::init())
        // The commands the apps share (`host.voice`, `host.engine`, `host.microphone`, `host.pairings`): who may call is
        // the app's one window, on its own page.
        .plugin(sidevoice_shared_tauri::init(the_page, pages))
        .manage(LastReport::default())
        .invoke_handler(tauri::generate_handler![
            audio_route,
            call_page,
            call_state,
            call_still,
            call_forget,
            cue,
            developer_options,
            global_shortcuts,
            screen_locked
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            // The log's file (and its folder), then where the panics' file goes, and the last run's panics into the log (`panics`).
            open_log_file(&handle);
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "app started");
            match handle.path().app_log_dir() {
                Ok(dir) => panics::watch(&dir),
                Err(_) => tracing::warn!("panics: no log folder"),
            }
            // The headset's buttons and the lock screen's controls act on the call through the page, as the desktop's do,
            // and on the room's call's thread, in order with its reports and interruptions (`call::RoomCall`).
            let media_app = handle.clone();
            media_controls::attach(
                &handle,
                Arc::new(move |command| match command {
                    // The single press (play/pause, the AirPods stem): what the person chose, the mute by default.
                    MediaCommand::ToggleMute => headset::pressed(&media_app, sidevoice_shared_headset::Presses::Single),
                    // The double press (the next-track command; iOS, while the double press has an action, and Android's skip to
                    // next): what the person chose, nothing by default.
                    MediaCommand::NextTrack => headset::pressed(&media_app, sidevoice_shared_headset::Presses::Double),
                }),
            );
            // A change of the audio route (AirPods in, the car's Bluetooth out) is told to the page, which asks `audio_route`
            // `current` for the name (`host.audioRoute.subscribe`).
            let route_app = handle.clone();
            if let Err(error) = platform::audio_route_picker::observe(Arc::new(move || {
                if let Some(window) = route_app.get_webview_window(WINDOW) {
                    let _ = window.eval("window.dispatchEvent(new Event('sidevoice-audio-route'));");
                }
            })) {
                log::warn!("audio route: {error}");
            }
            // Before the engine's first HTTPS request. Without it, remote providers and downloads fail; the rest runs.
            if let Err(error) = platform::tls::init() {
                log::error!("tls: {error}");
            }
            // The creatures' stills an earlier run kept for the call's surfaces: the page sends the ones it shows again.
            if let Err(error) = sidevoice_mobile_plugin::clear_stills(&handle) {
                log::warn!(target: "stills", "call_still: the earlier stills were not removed (os error {:?})", error.raw_os_error());
            }
            // Declared now, activated only while a call's audio is open (`call::PhoneCall`).
            if let Err(error) = call::configure() {
                log::error!("audio session: {error}");
            }
            let phone = Arc::new(call::PhoneCall::new(handle.clone()));
            // The developer options (nightly and dev builds): read before the first call, whose kind they decide.
            app.manage(developer_options::DeveloperOptions::of_app(
                &handle,
                env!("SIDEVOICE_BUILD_CHANNEL"),
            )?);
            app.manage(headset::HeadsetSettings::of_app(&handle)?);
            app.manage(Pairings(Arc::new(pairings::PairingFile::of_app(&handle)?)));
            // The application host: the voice call and the engine. A phone never resets its data under the call, so
            // every request is admitted.
            app.manage(AppHost::new(
                HostConfig {
                    model_root: model_root(&handle)?,
                },
                tauri::async_runtime::handle().inner().clone(),
                Hooks {
                    admission: Arc::new(AlwaysAdmit),
                    events: Arc::new(events.clone()),
                    keys: Arc::new(Keychain::new(PROVIDER_KEYS)),
                    // The voice calls' audio holds the audio session (iOS).
                    calls: phone.clone(),
                    // The screen stays on while a model download runs: the system suspends the app soon after it turns off.
                    downloads: Arc::new(downloads::KeepScreenOn::new(handle.clone())),
                    // The microphone is the audio session's route on iOS (its available and preferred inputs).
                    inputs: platform::audio_input::route(),
                },
            ));
            // The room's call holds the call's service (Android), for the host's current page.
            let page = app.state::<AppHost>().page().0;
            app.manage(call::RoomCall::new(handle.clone(), page, phone.clone()));
            // The system taking the audio away (a phone call, an alarm) pauses it; iOS tells the app here, Android through the
            // call's service.
            call::watch_interruptions(&handle, phone);
            let page_app = handle.clone();
            WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App(BUNDLED_INTERFACE.into()))
                // The page's console warnings and errors, uncaught errors and unhandled rejections, into the app's log (as
                // the desktop's room): the page's lines are the private `app` category of the unified log (`system_log`).
                .initialization_script(sidevoice_shared_tauri::js::WEBVIEW_LOG)
                .initialization_script(bridge::script(
                    APP_ORIGIN,
                    sidevoice_shared_tauri::platform::microphone::settings_available(),
                    bridge::microphone_help().as_ref(),
                    &bridge::messages(),
                    bridge::device_name().as_deref(),
                    &bridge::build(
                        env!("SIDEVOICE_BUILD_CHANNEL"),
                        &app.package_info().version.to_string(),
                        env!("SIDEVOICE_BUILD_COMMIT"),
                        env!("SIDEVOICE_BUILD_DATE"),
                    ),
                    app.state::<developer_options::DeveloperOptions>()
                        .state()
                        .as_ref(),
                ))
                // Only the app's own pages: the interface never navigates away.
                .on_navigation(|url| navigation_allowed(url, APP_ORIGIN))
                // No page captures anything (the microphone, the camera, the screen): the app runs the call. The webview
                // would grant every request when the app sets no handler.
                .on_permission_request(|_webview, _kind| PermissionResponse::Deny)
                // A page that loads anew owns none of the calls of the one before, and hears nothing of them.
                .on_page_load(move |webview, payload| {
                    if payload.event() == PageLoadEvent::Finished {
                        // The window is on screen by now: where the web view really is (the insets are 0 before).
                        let _ = webview.with_webview(|webview| {
                            platform::webview::fill_screen(&webview);
                            platform::webview::log_layout("page loaded", &webview)
                        });
                        // The page has drawn by then: where everything is once it has settled.
                        let later = webview.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_secs(8));
                            let _ = later.with_webview(|webview| {
                                platform::webview::log_layout("8 s after the page loaded", &webview)
                            });
                        });
                    }
                    if payload.event() == PageLoadEvent::Started {
                        if let Some(host) = page_app.try_state::<AppHost>() {
                            // A page that loads anew is in no call until it says so: the room's call of the one before
                            // ends, queued now, before anything the new page reports (`call::RoomCall`).
                            let page = host.page_changed().0;
                            let room = page_app.state::<call::RoomCall>();
                            room.page_changed(page);
                        }
                        *page_app.state::<LastReport>().0.lock().unwrap() = CallReport::default();
                    }
                })
                .build()?
                // The page owns the safe areas: the web view fills the screen, on the system's background (`platform::webview`).
                .with_webview(|webview| {
                    platform::webview::fill_screen(&webview);
                    platform::webview::log_layout("at start", &webview);
                })?;
            Ok(())
        })
        .build(context)
        .expect("error while building the Sidevoice mobile app")
        .run(|app, event| {
            // A link the app is opened with, from the call's surfaces (iOS: the Live Activity's stack,
            // `sidevoice://conversations`): run on the page's call.
            if let tauri::RunEvent::Opened { urls } = &event {
                let actions = urls
                    .iter()
                    .filter_map(|url| SurfaceAction::from_url(url.as_str()));
                for action in actions {
                    if let Some(room) = app.try_state::<call::RoomCall>() {
                        room.surface_action(action);
                    }
                }
            }
            // Quitting: the calls stop and every model and audio device is let go, bounded.
            if let tauri::RunEvent::Exit = event {
                let deadline = std::time::Instant::now() + RELEASE_LIMIT;
                if let Some(room) = app.try_state::<call::RoomCall>() {
                    room.end(deadline);
                }
                if let Some(host) = app.try_state::<AppHost>() {
                    tauri::async_runtime::block_on(host.close(
                        deadline.saturating_duration_since(std::time::Instant::now()),
                    ));
                }
            }
        });
}

/// The app log's file, in the platform's log folder (`app_log_dir`; `panics` keeps its file beside it): rotated at
/// [`LOG_MAX_BYTES`], keeping [`LOG_KEPT`] files.
const LOG_MAX_BYTES: u128 = 2_000_000;
const LOG_KEPT: usize = 5;

/// The file logger of the app log, held once `setup` knows the log folder (`app_log_dir` needs the app). The subscriber's file
/// layer hands it each finished line.
static LOG_SINK: std::sync::OnceLock<Box<dyn log::Log>> = std::sync::OnceLock::new();

/// The lines written before [`LOG_SINK`] exists (the first `EARLY_LINES`), kept for [`open_log_file`] to write.
static EARLY: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
const EARLY_LINES: usize = 200;

fn write_line(file: &dyn log::Log, line: &str) {
    file.log(
        &log::Record::builder()
            .args(format_args!("{line}"))
            .level(log::Level::Info)
            .target("sidevoice")
            .build(),
    );
}

/// tauri-plugin-log's file logger, built but not installed (`split`): it keeps the file's place and rotation.
fn open_log_file(app: &AppHandle) {
    use tauri_plugin_log::{FileOpenStrategy, RotationStrategy, Target, TargetKind};
    let built = tauri_plugin_log::Builder::new()
        .clear_targets()
        .target(Target::new(TargetKind::LogDir { file_name: None }))
        .format(|out, message, _| out.finish(*message))
        .max_file_size(LOG_MAX_BYTES)
        .rotation_strategy(RotationStrategy::KeepSome(LOG_KEPT))
        .file_open_strategy(FileOpenStrategy::Rotate)
        .split(app);
    if let Ok((_, _, logger)) = built {
        // Under the buffer's lock, so a line is either in the buffer to flush or written by the sink, never lost.
        let mut early = EARLY.lock().unwrap_or_else(|e| e.into_inner());
        if LOG_SINK.set(logger).is_ok() {
            if let Some(file) = LOG_SINK.get() {
                for line in early.drain(..) {
                    write_line(file.as_ref(), &line);
                }
            }
        }
    }
}

/// The targets a bare `SIDEVOICE_LOG` level applies to: our crates and the fixed targets (`panic`, the page's `webview`, the
/// system log's four categories). Every other target stays at `warn` unless the variable names it.
const OWN_TARGETS: &[&str] = &[
    "sidevoice_mobile_lib",
    "sidevoice_mobile_plugin",
    "sidevoice_shared",
    "sidevoice_shared_tauri",
    "sidevoice_shared_i18n",
    "sidevoice_shared_headset",
    "sidevoice_voice",
    "sidevoice_engine",
    "panic",
    "webview",
    "stills",
    "intent",
    "cue",
    "audio",
];

/// The process's subscriber, first thing in `run_app`: the file layer, and the system's. No layer writes to stderr (on iOS
/// that is the system's public log), and the panic hook is [`panics`]', which never logs. `run_app` runs again when Android
/// recreates the activity: the second call finds the subscriber installed and does nothing.
fn init_logging() {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let system = system_log::SystemLog::new(
        sidevoice_shared::logging::Home::new(home.as_deref()),
        platform::system_log::write,
    );
    sidevoice_shared::logging::init_with(
        OWN_TARGETS,
        home.as_deref(),
        |line: &str| {
            let mut early = EARLY.lock().unwrap_or_else(|e| e.into_inner());
            match LOG_SINK.get() {
                Some(file) => write_line(file.as_ref(), line),
                None if early.len() < EARLY_LINES => early.push(line.to_owned()),
                None => {}
            }
        },
        sidevoice_shared::logging::Extras {
            layer: Some(system),
            stderr: false,
            panic_hook: false,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(url: &str, app_origin: &str) -> bool {
        navigation_allowed(&Url::parse(url).unwrap(), app_origin)
    }

    #[test]
    fn navigation_stays_on_the_app_s_pages() {
        for app in ["tauri://localhost", "http://tauri.localhost"] {
            let page = format!("{app}/voice/index.html");
            assert!(allowed(&page, app), "{page}");
            for url in ["about:blank", "blob:null/1b2c", "data:text/html,x"] {
                assert!(allowed(url, app), "{url}");
            }
        }
        for url in [
            "https://evil.example/",
            "http://127.0.0.1:8768/",
            "http://192.168.1.20:8768/",
            "file:///etc/passwd",
            "tauri://other/",
            "http://tauri.localhost:8080/",
            "ipc://localhost/",
            "javascript:alert(1)",
        ] {
            assert!(
                !allowed(url, "tauri://localhost") && !allowed(url, "http://tauri.localhost"),
                "{url}"
            );
        }
        // Each OS has one app origin: the other one's pages are not its own.
        assert!(!allowed("http://tauri.localhost/", "tauri://localhost"));
        assert!(!allowed("tauri://localhost/", "http://tauri.localhost"));
    }

    #[test]
    fn only_the_app_s_window_on_its_own_page_may_call() {
        assert_eq!(is_the_page(WINDOW, Some(APP_ORIGIN)), Ok(()));
        for label in ["room-1", "other", ""] {
            assert_eq!(
                is_the_page(label, Some(APP_ORIGIN)),
                Err("not the app's window".into()),
                "{label}"
            );
        }
        for origin in [
            Some("https://example.com"),
            Some("http://tauri.localhost.evil.test"),
            Some("null"),
            Some(""),
            None,
        ] {
            assert_eq!(
                is_the_page(WINDOW, origin),
                Err("not the app's own page".into()),
                "{origin:?}"
            );
        }
        let other = if APP_ORIGIN == "tauri://localhost" {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        };
        assert!(
            is_the_page(WINDOW, Some(other)).is_err(),
            "the other OS's origin is not this one's"
        );
    }

    #[test]
    fn the_models_stay_out_of_android_backups_and_transfers() {
        let rule = format!("<exclude domain=\"file\" path=\"{MODELS}/\" />");
        let old = include_str!("../plugin/android/src/main/res/xml/backup_rules.xml");
        let new = include_str!("../plugin/android/src/main/res/xml/data_extraction_rules.xml");
        assert_eq!(old.matches(&rule).count(), 1, "backup_rules.xml");
        assert_eq!(
            new.matches(&rule).count(),
            2,
            "data_extraction_rules.xml: the cloud backup and the transfer"
        );
    }
}
