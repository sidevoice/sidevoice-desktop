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
mod call;
mod developer_options;
mod downloads;
mod hold;
mod i18n;
pub mod pairings;
mod platform;
mod surface;

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

/// How long quitting waits for the calls to stop and the models to be released (as the desktop).
const RELEASE_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

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
    the_page(&app, &webview)?;
    if png.len() > sidevoice_mobile_plugin::STILL_LIMIT / 3 * 4 + 4 {
        return Err("call_still: larger than a still".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png.as_bytes())
        .map_err(|_| "call_still: not base64".to_owned())?;
    tauri::async_runtime::spawn_blocking(move || {
        let room = app.state::<call::RoomCall>();
        room.save_still(page, id, bytes, px)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| {
        log::warn!("call_still: not kept: {error}");
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
            now_playing_off: false,
        }
    }
}

/// The app: one window, on the bundled interface, with the members the apps share.
fn run_app() {
    // Where the call's events go: over the channel the page opens (the shared plugin's `voice_events`).
    let pages = Pages::default();
    let events = pages.clone();
    tauri::Builder::default()
        // The app's log: the system's (logcat on Android, the unified log on iOS) and a file in the app's log folder, at
        // `info` as on the desktop (the plugin's default is `trace`, every crate's every line, written synchronously to
        // both while the call runs), with the window's own crates at `warn`.
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .level_for("tao", log::LevelFilter::Warn)
                .level_for("wry", log::LevelFilter::Warn)
                .build(),
        )
        .plugin(sidevoice_mobile_plugin::init())
        // The commands the apps share (`host.voice`, `host.engine`, `host.microphone`, `host.pairings`): who may call is
        // the app's one window, on its own page.
        .plugin(sidevoice_shared_tauri::init(the_page, pages))
        .manage(LastReport::default())
        .invoke_handler(tauri::generate_handler![
            call_page,
            call_state,
            call_still,
            call_forget,
            developer_options
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            // The headset's buttons and the lock screen's controls act on the call through the page, as the desktop's do,
            // and on the room's call's thread, in order with its reports and interruptions (`call::RoomCall`).
            let media_app = handle.clone();
            media_controls::attach(
                &handle,
                Arc::new(move |command| match command {
                    MediaCommand::ToggleMute => {
                        if let Some(room) = media_app.try_state::<call::RoomCall>() {
                            room.toggle_mute();
                        }
                    }
                }),
            );
            // Before the engine's first HTTPS request. Without it, remote providers and downloads fail; the rest runs.
            if let Err(error) = platform::tls::init() {
                log::error!("tls: {error}");
            }
            // Declared now, activated only while a call's audio is open (`call::PhoneCall`).
            if let Err(error) = call::configure() {
                log::error!("audio session: {error}");
            }
            // The creatures' stills an earlier run kept for the call's surfaces: the page sends the ones it shows again.
            if let Err(error) = sidevoice_mobile_plugin::clear_stills(&handle) {
                log::warn!("call_still: the earlier stills were not removed: {error}");
            }
            let phone = Arc::new(call::PhoneCall::default());
            // The developer options (nightly and dev builds): read before the first call, whose kind they decide.
            app.manage(developer_options::DeveloperOptions::of_app(
                &handle,
                env!("SIDEVOICE_BUILD_CHANNEL"),
            )?);
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
        .build(tauri::generate_context!())
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
                if let Some(room) = app.try_state::<call::RoomCall>() {
                    room.end(RELEASE_LIMIT);
                }
                if let Some(host) = app.try_state::<AppHost>() {
                    tauri::async_runtime::block_on(host.close(RELEASE_LIMIT));
                }
            }
        });
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
