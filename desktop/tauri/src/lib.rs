#![cfg(desktop)]
//! Sidevoice desktop: a Tauri shell around the Sidevoice web interface, bundled in the app, whose tray drives
//! the call through the interface's own actions (docs/BRIDGE.md).
//!
//! The interface is a secret-free client: it talks to a node it was paired with (the node
//! issues a one-time code; sidevoice-core `control/devices.py`), and the app never tells it where to look
//! (docs/TARGETS.md). No remote page is ever loaded.
//!
//! Windows:
//! - `room-N`: the bundled interface. Created at launch, and again (N grows) when it is gone. Closing it hides it; the
//!   app keeps running.
//! - `call-controls`: the call controls card (`src/call_controls.rs`), floating over other apps during a call.

#[cfg(test)]
mod acl;
mod app_hooks;
mod call_controls;
mod data;
mod debug_options;
mod legacy_identifier;
mod local_host;
pub(crate) mod media_duck;
/// A look at the process's own memory, for CI's measurements of it (#145). Never in a release.
#[cfg(any(test, feature = "probe"))]
mod memory;
mod platform;
#[cfg(feature = "probe")]
mod probe;
mod scripts;
mod shortcuts;
mod tray;
mod updater;
mod voice;

use crate::platform::application;
use crate::platform::diagnostics;
use crate::platform::host;
use crate::platform::shortcut_access;
use serde_json::{json, Value};
use sidevoice_desktop_core::bridge::{self, AgentState, CallSnapshot, Command};
use sidevoice_desktop_core::build_info::BuildInfo;
use sidevoice_desktop_core::settings::Settings;
use sidevoice_desktop_core::shortcuts::Action;
use sidevoice_desktop_core::{media, settings};
use sidevoice_shared::app::{AppHost, Hooks, HostConfig, NoCallActivity, NoDownloadActivity, NoInputRoute};
use sidevoice_shared_tauri::platform::media_controls::{self, CallState, MediaCommand};
use sidevoice_shared_tauri::platform::origin::APP_ORIGIN;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tauri::utils::config::BackgroundThrottlingPolicy;
use tauri::webview::{PageLoadEvent, PermissionResponse};
use tauri::{AppHandle, Manager, State, Webview, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

const ROOM_PREFIX: &str = "room-";
/// The web interface bundled in the app (the sidevoice-web release `web.pin.json` names, `scripts/build-web.mjs`), at
/// the path a room serves it from, so its absolute `/voice/…` URLs resolve.
const BUNDLED_INTERFACE: &str = "voice/index.html";

pub(crate) struct AppState {
    pub(crate) settings: Mutex<Option<Settings>>,
    pub(crate) call: Mutex<CallSnapshot>,
    room_label: Mutex<Option<String>>,
    room_counter: AtomicU32,
    /// The language of the app's native texts (tray, …), the system's when there is a bundle for it (core `i18n`).
    pub(crate) language: &'static str,
    resetting: std::sync::atomic::AtomicBool,
    reset_admission: Mutex<data::Admission>,
    reset_helper: Mutex<Option<platform::reset_handoff::Pending>>,
}

impl Default for AppState {
    fn default() -> Self {
        let preferred = platform::locale::preferred();
        AppState {
            settings: Mutex::default(),
            call: Mutex::default(),
            room_label: Mutex::default(),
            room_counter: AtomicU32::default(),
            language: sidevoice_desktop_core::i18n::language(&preferred),
            resetting: std::sync::atomic::AtomicBool::new(false),
            reset_admission: Mutex::default(),
            reset_helper: Mutex::default(),
        }
    }
}

/// The one writer of settings.json: `change` applied to the stored settings, saved and kept under one lock, so the
/// room page's setters never lose each other's fields.
pub(crate) fn change_settings(app: &AppHandle, change: impl FnOnce(&mut Settings)) -> Result<Settings, String> {
    let dir = data::desktop_dir(app).map_err(|e| e.to_string())?;
    let state = app.state::<AppState>();
    let mut stored = state.settings.lock().unwrap();
    let mut changed = stored.clone().unwrap_or_else(Settings::first_run);
    change(&mut changed);
    settings::save(&dir, &changed).map_err(|e| e.to_string())?;
    *stored = Some(changed.clone());
    Ok(changed)
}

/// The choice about other apps' media and how far Lower goes, as kept.
pub(crate) fn stored_media_duck(
    app: &AppHandle,
) -> (sidevoice_desktop_core::media_duck::Mode, sidevoice_desktop_core::media_duck::Level) {
    app.state::<AppState>()
        .settings
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| (s.media_duck, s.media_duck_level))
        .unwrap_or_default()
}

/// A refusal as the room page reads it (docs/BRIDGE.md → "Refusals").
pub(crate) fn refusal(key: &str, message: impl std::fmt::Display) -> Value {
    json!({ "key": key, "message": message.to_string() })
}

/// `host.shortcuts`' one command (and `host.muteShortcut`'s, which is its mute shortcut): `action` `get`; `set` the
/// shortcut named by `target` to `accelerator` (`null`: none) or `restore` it to this system's default; `request` the `permission` a single key needs; `headset`
/// with what to change; or `record` the next key for `target`. Refuses `shortcut_invalid` for an accelerator that is not a
/// shortcut the system can read, or that its action cannot be done with.
#[tauri::command]
async fn global_shortcuts(
    app: AppHandle,
    webview: Webview,
    action: String,
    target: Option<String>,
    accelerator: Option<String>,
    permission: Option<String>,
    headset: Option<Value>,
) -> Result<Value, Value> {
    room_page(&app, &webview).map_err(|why| refusal("bad_request", why))?;
    let named = || {
        target
            .as_deref()
            .and_then(Action::parse)
            .ok_or_else(|| refusal("bad_request", format!("not a shortcut action: {target:?}")))
    };
    match action.as_str() {
        "get" => Ok(json!(shortcuts::status(&app))),
        "set" => shortcuts::set(&app, named()?, &accelerator.unwrap_or_default()).map(|shortcut| json!(shortcut)),
        "restore" => shortcuts::restore(&app, named()?).map(|shortcut| json!(shortcut)),
        "request" => shortcuts::request(&app, permission.as_deref()).map(|status| json!(status)),
        "headset" => {
            let change = headset.unwrap_or(Value::Null);
            shortcuts::set_headset(
                &app,
                change.get("press").and_then(Value::as_str),
                change.get("doublePress"),
                change.get("outsideCall").and_then(Value::as_str),
            )
            .map(|status| json!(status))
        }
        "record" => shortcuts::record(app.clone(), named()?).await.map(|accelerator| json!(accelerator)),
        _ => Err(refusal("bad_request", format!("not a shortcuts action: {action}"))),
    }
}

/// Called by the bridge script in the interface. Only the current room window, on the app's own pages, may
/// call it (capabilities/room.json + the check below).
#[tauri::command]
fn bridge_state(
    app: AppHandle,
    webview: Webview,
    state: State<'_, AppState>,
    snapshot: CallSnapshot,
) -> Result<(), String> {
    let current_label = state.room_label.lock().unwrap().clone();
    if current_label.as_deref() != Some(webview.label()) {
        debug(&format!("bridge_state refused: window {}", webview.label()));
        tracing::warn!(reason = "not_room_window", "bridge_state refused");
        return Err("not the room window".into());
    }
    let actual = webview.url().map(|u| settings::url_origin(&u)).ok();
    if actual.as_deref() != Some(APP_ORIGIN) {
        debug(&format!("bridge_state refused: origin {actual:?}"));
        tracing::warn!(reason = "not_app_page", "bridge_state refused");
        return Err("not the app's own page".into());
    }
    let previous = std::mem::replace(&mut *state.call.lock().unwrap(), snapshot.clone());
    if let Some(changes) = snapshot.changes_since(&previous) {
        tracing::info!(changes, "bridge_state");
    }
    // The whole snapshot only on stderr with SIDEVOICE_DEBUG=1 (never the log), without the rest of the room's projection:
    // its facts carry the conversation's words.
    debug_detail(|| format!("bridge_state {:?}", CallSnapshot { rest: Default::default(), ..snapshot.clone() }));
    tray::update(&app, &snapshot);
    media_controls::update(&app, &media_state(&snapshot, shortcuts::headset_press_keeps_mic(&app)));
    media_duck::call(&app, snapshot.joined, snapshot.mic_enabled, snapshot.agent == AgentState::Speaking);
    shortcuts::call_changed(&app, snapshot.joined);
    call_controls::update(&app);
    Ok(())
}

/// Whether `webview` is the current room window showing the app's own page: the only caller the room's commands
/// accept (`local_host_*`; capabilities/room.json grants them to `room-*`, this narrows to the current
/// one and its origin).
pub(crate) fn room_page(app: &AppHandle, webview: &Webview) -> Result<(), String> {
    let current_label = app.state::<AppState>().room_label.lock().unwrap().clone();
    let origin = webview.url().map(|u| settings::url_origin(&u)).ok();
    is_room_page(current_label.as_deref(), webview.label(), origin.as_deref())
}

/// [`room_page`]'s answer for a webview called `label` showing a page of `origin`, when `current` is the room window's label.
fn is_room_page(current: Option<&str>, label: &str, origin: Option<&str>) -> Result<(), String> {
    if current != Some(label) {
        return Err("not the room window".into());
    }
    if origin != Some(APP_ORIGIN) {
        return Err("not the app's own page".into());
    }
    Ok(())
}

/// The app log: `<LOG_FILE>.log` in the platform's log directory (macOS `~/Library/Logs/<identifier>/`, Linux
/// `~/.local/share/<identifier>/logs/`, Windows `%LOCALAPPDATA%\<identifier>\logs\`), with the Rust side's lines and the
/// room page's (shared/tauri/js/webview-log.js). It is rotated at [`LOG_MAX_BYTES`], keeping [`LOG_KEPT`] files.
pub(crate) const LOG_FILE: &str = "sidevoice-desktop";
const LOG_MAX_BYTES: u128 = 2_000_000;
const LOG_KEPT: usize = 5;

/// Where the app log is (it may not exist yet).
pub(crate) fn app_log_file(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_log_dir().ok().map(|dir| dir.join(format!("{LOG_FILE}.log")))
}

/// The webview's `log` command only (its lines have target `webview`): the plugin installs no logger. Its records reach the
/// subscriber through `log`'s bridge, so one filter and one writer serve every line.
fn log_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri_plugin_log::Builder::new().skip_logger().build()
}

/// The file logger of the app log, held once `setup` knows the log folder (`app_log_dir` needs the app). The subscriber's
/// file layer hands it each finished line.
static LOG_SINK: std::sync::OnceLock<Box<dyn log::Log>> = std::sync::OnceLock::new();

/// The lines written before [`LOG_SINK`] exists (the first `EARLY_LINES`), kept for [`open_log_file`] to write.
static EARLY: Mutex<Vec<String>> = Mutex::new(Vec::new());
const EARLY_LINES: usize = 200;

fn write_line(file: &dyn log::Log, line: &str) {
    file.log(&log::Record::builder().args(format_args!("{line}")).level(log::Level::Info).target("sidevoice").build());
}

/// tauri-plugin-log's file logger, built but not installed (`split`): it keeps the file's name, place and rotation.
fn open_log_file(app: &AppHandle) {
    use tauri_plugin_log::{FileOpenStrategy, RotationStrategy, Target, TargetKind};
    let built = tauri_plugin_log::Builder::new()
        .clear_targets()
        .target(Target::new(TargetKind::LogDir { file_name: Some(LOG_FILE.into()) }))
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

/// The targets a bare `SIDEVOICE_LOG` level applies to: our crates and the fixed targets (`panic`, the page's `webview`).
/// Every other target stays at `warn` unless the variable names it.
const OWN_TARGETS: &[&str] = &[
    "sidevoice_desktop_lib",
    "sidevoice_desktop_core",
    "sidevoice_local_host",
    "sidevoice_shared",
    "sidevoice_shared_tauri",
    "sidevoice_shared_i18n",
    "sidevoice_shared_headset",
    "sidevoice_voice",
    "sidevoice_engine",
    "sidevoice_platform",
    "panic",
    "webview",
];

/// The process's subscriber, first thing in `run`.
fn init_logging() {
    sidevoice_shared::logging::init(OWN_TARGETS, platform::account::home().ok().as_deref(), |line: &str| {
        // In the panic hook the thread may already hold the buffer: only try.
        let early = if sidevoice_shared::logging::in_panic_hook() {
            EARLY.try_lock().ok()
        } else {
            Some(EARLY.lock().unwrap_or_else(|e| e.into_inner()))
        };
        match (LOG_SINK.get(), early) {
            (Some(file), _) => write_line(file.as_ref(), line),
            (None, Some(mut early)) if early.len() < EARLY_LINES => early.push(line.to_owned()),
            _ => {}
        }
    });
}

// Diagnostics on stderr with `SIDEVOICE_DEBUG=1` (CI's smoke tests read them): sidevoice-shared's, shared with the
// call host.
#[cfg(feature = "probe")]
use sidevoice_shared::debug::enabled as debugging;
pub(crate) use sidevoice_shared::debug::{debug, debug_detail};

/// A line from a page's diagnostics (the CI probe), printed only with `SIDEVOICE_DEBUG=1`. Only a probe build reads it: in any
/// other the command does nothing, so page text never reaches a log at any level.
#[tauri::command]
fn debug_log(app: AppHandle, line: String) {
    #[cfg(feature = "probe")]
    {
        debug(&format!("page says: {}", line.chars().take(2000).collect::<String>()));
        probe::exit_on(&app, &line);
        probe::memory_on(&app, &line);
    }
    let _ = (app, line);
}

/// `host.screen.locked()` (docs/BRIDGE.md -> "The screen"): whether the person's screen is locked now, for the page's "only
/// with the screen off" thinking sound; the room's window hidden is not the screen off (the call goes on in the floating card).
/// `false` where this system has no observation the app reads (`platform::session`: Linux, for now).
#[tauri::command]
fn screen_locked(app: AppHandle, webview: Webview) -> Result<bool, Value> {
    room_page(&app, &webview).map_err(|why| refusal("not_room_page", why))?;
    Ok(platform::session::screen_locked())
}

pub(crate) fn room_window(app: &AppHandle) -> Option<WebviewWindow> {
    let label = app.state::<AppState>().room_label.lock().unwrap().clone()?;
    app.get_webview_window(&label)
}

/// Sends one command to the room page. Works with the window hidden.
pub(crate) fn send(app: &AppHandle, command: Command) {
    if let Some(w) = room_window(app) {
        let _ = w.eval(command.script());
    }
}

/// What the headset buttons and Now Playing need of the call the room reported, and of the single press the person chose
/// (`press_keeps_mic`: anything but the mute, which the AirPods gesture must not tell the system was a mute).
fn media_state(snapshot: &CallSnapshot, press_keeps_mic: bool) -> CallState {
    CallState {
        ready: snapshot.ready,
        joined: snapshot.joined,
        mic_enabled: snapshot.mic_enabled,
        title: snapshot.title.clone(),
        double_press_wanted: false,
        press_keeps_mic,
        // macOS has no record session to wait for: the opt-in at launch is the one there.
        record_active: false,
    }
}

/// The headset's media controls told the call again, after the person changed what the single press does.
pub(crate) fn refresh_media_controls(app: &AppHandle) {
    let snapshot = app.state::<AppState>().call.lock().unwrap().clone();
    media_controls::update(app, &media_state(&snapshot, shortcuts::headset_press_keeps_mic(app)));
}

fn reset_call_state(app: &AppHandle) {
    let snapshot = CallSnapshot::default();
    *app.state::<AppState>().call.lock().unwrap() = snapshot.clone();
    tray::update(app, &snapshot);
    media_controls::update(app, &media_state(&snapshot, shortcuts::headset_press_keeps_mic(app)));
    media_duck::call(app, false, false, false);
    shortcuts::call_changed(app, false);
    call_controls::update(app);
    voice::page_changed(app);
}

/// (Re)creates the room window: the bundled interface, bound to the app's own pages.
fn open_room(app: &AppHandle) -> tauri::Result<()> {
    // Reset captures browser writers before exit. A tray/reopen action must not create another
    // WebKit/WebView2 process after that snapshot (or race reset admission while building it).
    let Some(_permit) = data::admit(app) else { return Ok(()) };
    let state = app.state::<AppState>();
    let label = format!("{ROOM_PREFIX}{}", state.room_counter.fetch_add(1, Ordering::SeqCst) + 1);
    #[cfg(not(feature = "probe"))]
    let page = BUNDLED_INTERFACE;
    // CI's probe build only (src/probe.rs): the probe page may take the interface's place in this same window.
    #[cfg(feature = "probe")]
    let page = probe::page(BUNDLED_INTERFACE);

    if let Some(old) = room_window(app) {
        old.destroy()?;
    }
    *state.room_label.lock().unwrap() = Some(label.clone());
    reset_call_state(app);

    let page_app = app.clone();
    // `host.build`, as xtask built this copy (build.rs).
    let version = app.package_info().version.to_string();
    let (commit, date) = (env!("SIDEVOICE_BUILD_COMMIT"), env!("SIDEVOICE_BUILD_DATE"));
    let build = BuildInfo::new(env!("SIDEVOICE_BUILD_CHANNEL"), &version, commit, date);
    let debug_options = app.state::<AppState>().settings.lock().unwrap().as_ref().map(|s| s.debug).unwrap_or_default();
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(page.into()))
        .title("Sidevoice")
        .inner_size(1100.0, 760.0)
        .min_inner_size(420.0, 560.0)
        .initialization_script(scripts::NO_CONTEXT_MENU)
        // The room page's console warnings and errors, uncaught errors and unhandled rejections, into the app log.
        .initialization_script(sidevoice_shared_tauri::js::WEBVIEW_LOG)
        .initialization_script(bridge::script_for_origin(
            APP_ORIGIN,
            host::updates_available(&build.channel),
            &build,
            host::name().as_deref(),
            &debug_options,
            sidevoice_shared_tauri::platform::microphone::settings_available(),
            &diagnostics::webview_switches(),
            media_controls::available(),
            platform::uninstall::available(),
            host::shortcut_modifier(),
            platform::key_capture::captures(),
            &host::microphone_help(),
            &sidevoice_shared_tauri::messages::tables(&[]),
            &bridge::Parts {
                transport: sidevoice_shared_tauri::js::TRANSPORT,
                members: sidevoice_shared_tauri::js::MEMBERS,
                desktop: scripts::DESKTOP_BRIDGE,
            },
        ));
    #[cfg(feature = "probe")]
    let builder = match probe::local_host_flow() {
        Some(flow) => builder.initialization_script(flow),
        None => builder,
    };
    builder
        // A hidden window still carries the call: never suspend or throttle its page.
        .background_throttling(BackgroundThrottlingPolicy::Disabled)
        // Only the app's own pages: the interface never navigates away (a link elsewhere goes nowhere).
        .on_navigation(|url| media::navigation_allowed(url.scheme(), &settings::url_origin(url), APP_ORIGIN))
        // No page captures anything: the app runs the voice call (`host.voice`, src/voice.rs) and owns the microphone.
        // wry grants every request when the app sets no handler; this one denies the microphone, the camera and the rest.
        .on_permission_request(|_webview, kind| {
            debug(&format!("media {kind:?}: denied"));
            tracing::warn!(kind = ?kind, "media permission denied");
            PermissionResponse::Deny
        })
        .on_page_load(move |_webview, payload| {
            // `Started` is WebKit's didCommitNavigation: after redirects, main frame only.
            if payload.event() == PageLoadEvent::Started {
                debug(&format!("page {}", url_path(payload.url())));
                tracing::info!(url = %url_path(payload.url()), "page");
                reset_call_state(&page_app);
            }
        })
        .build()?;
    debug_options::apply(app);
    Ok(())
}

pub(crate) fn show_room(app: &AppHandle) {
    match room_window(app) {
        Some(w) => {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
        None => {
            let _ = open_room(app);
        }
    }
}

/// The tray's Settings: the room in front, on its own Settings (the page opens them where they were left).
pub(crate) fn show_settings(app: &AppHandle) {
    show_room(app);
    send(app, Command::ShowSettings);
}

/// `SIDEVOICE_NO_CALL_CARD=1`: the app starts without the call controls card, to tell whether the card is what takes
/// the Dock tile away on macOS (#117). A diagnostic switch: nothing else changes, the tray's card item does nothing.
fn call_card_disabled() -> bool {
    std::env::var_os("SIDEVOICE_NO_CALL_CARD").is_some_and(|v| v == "1")
}

/// Native lifecycle observations plus the common diagnostic card setting (#117).
fn app_report(app: &AppHandle) -> String {
    format!("{} call_card={}", application::report(app), if call_card_disabled() { "off" } else { "on" })
}

/// Every command, the voice call's included (`host.voice`, src/voice.rs).
fn handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        bridge_state,
        call_controls::call_controls_ready,
        call_controls::call_controls_run,
        call_controls::call_controls_layout,
        call_controls::call_controls_drag,
        call_controls::call_controls_resize,
        call_controls::call_controls_cursor,
        debug_log,
        local_host::local_host_state,
        local_host::local_host_pairing,
        local_host::local_host_version,
        local_host::local_host_action,
        local_host::local_host_pairing_code,
        local_host::local_host_install,
        local_host::local_host_install_progress,
        local_host::local_host_install_cancel,
        data::data_delete_all,
        data::data_uninstall,
        data::data_reset_report,
        global_shortcuts,
        media_duck::media_duck,
        call_controls::call_controls_settings,
        screen_locked,
        updater::updater,
        debug_options::debug_options
    ]
}

pub fn run() {
    use platform::reset_handoff::{self, Handoff};
    let startup = match Handoff::new().and_then(|handoff| {
        handoff.await_idle(std::time::Duration::from_secs(5 * 60), &std::sync::atomic::AtomicBool::new(false))
    }) {
        Ok(guard) => guard,
        Err(_) => {
            // Before the log and the app state exist: the system's language is read here.
            let preferred = platform::locale::preferred();
            let language = sidevoice_desktop_core::i18n::language(&preferred);
            #[allow(
                clippy::disallowed_macros,
                reason = "output for the person: the reset is busy, before any log exists"
            )]
            tell_the_person(sidevoice_desktop_core::i18n::t(language, "startup.reset_busy"));
            return;
        }
    };
    // First thing under the launch lock: before the log, and before anything reads the app's data, credentials or
    // webview store. It logs once the log exists (`setup`).
    let carried = legacy_identifier::carry_over();
    #[allow(clippy::disallowed_macros, reason = "the context Tauri generates prints its own diagnostics")]
    let context = tauri::generate_context!();
    #[cfg(feature = "probe")]
    let context = probe::with_page(context);
    #[cfg(feature = "probe")]
    probe::watch_exit_handlers();
    init_logging();
    // First, so everything after it is logged.
    // Where the call's events go: over the channel the room page opens (the shared plugin's `voice_events`).
    let pages = sidevoice_shared_tauri::Pages::default();
    let events = pages.clone();
    let builder = tauri::Builder::default().plugin(log_plugin());
    // The call controls card is a non-activating panel on macOS (src/call_controls.rs).
    let builder = <platform::card_window::NativeCardWindow as platform::card_window::CardWindow>::configure(builder);
    application::configure(builder)
        .plugin(tauri_plugin_opener::Builder::new().open_js_links_on_click(false).build())
        // The commands the apps share (`host.voice`, `host.engine`): who may call is this app's room page.
        .plugin(sidevoice_shared_tauri::init(room_page, pages))
        .manage(AppState::default())
        .invoke_handler(handler())
        .setup(move |app| {
            let handle = app.handle().clone();
            open_log_file(&handle);
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "app started");
            carried.log();
            application::prepare(&handle)?;
            shortcuts::setup(&handle);
            debug(&format!("launch {}", app_report(&handle)));
            tracing::info!(call_card = !call_card_disabled(), "app launch");
            tray::create(&handle)?;
            let media_app = handle.clone();
            media_controls::attach(
                &handle,
                Arc::new(move |command| match command {
                    // A headset's single press (and the AirPods gesture): what the person chose for it.
                    MediaCommand::ToggleMute => shortcuts::headset_press(&media_app),
                    // A headset's double press arrives as the next-track command.
                    MediaCommand::NextTrack => shortcuts::headset_double_press(&media_app),
                }),
            );
            // The model root is shared per OS user on this machine (docs/ENGINES.md).
            // Without it the app still runs; the page then finds no native engine to use.
            let engine_root =
                data::shared_model_root().ok_or_else(|| std::io::Error::other("no home for the shared model store"))?;
            // The application host: the voice call and the engine (`host.voice`, `host.engine`; src/voice.rs, src/engine_ipc.rs).
            // The engine failing to start is logged by the host, and the app goes on without one.
            app.manage(AppHost::new(
                HostConfig { model_root: engine_root },
                tauri::async_runtime::handle().inner().clone(),
                Hooks {
                    admission: Arc::new(app_hooks::ResetAdmission(handle.clone())),
                    events: Arc::new(events),
                    keys: Arc::new(app_hooks::keychain()),
                    calls: Arc::new(NoCallActivity),
                    downloads: Arc::new(NoDownloadActivity),
                    inputs: Arc::new(NoInputRoute),
                },
            ));
            // What the shared plugin's commands hand to the app: the levels to the card, the pairings to the private file.
            app.manage(sidevoice_shared_tauri::Observers(Arc::new(app_hooks::CardLevels(handle.clone()))));
            app.manage(sidevoice_shared_tauri::Pairings(Arc::new(data::Pairings(handle.clone()))));
            // This computer's own core, on every OS: found, paired, proxied for the page (docs/LOCAL_HOST.md).
            local_host::setup(&handle);
            let mut stored = data::desktop_dir(&handle).ok().and_then(|dir| settings::load(&dir));
            // A person still on the old mute default is moved to this system's, once (core `shortcuts::migrated_mute`).
            if let Some(kept) = stored.as_mut() {
                if kept.migrate_shortcuts(shortcut_access::default_mute()) {
                    if let Ok(dir) = data::desktop_dir(&handle) {
                        let _ = settings::save(&dir, kept);
                    }
                }
            }
            *app.state::<AppState>().settings.lock().unwrap() = stored.clone();
            // First run too: the interface itself asks for a pairing code; nothing has to be set up first.
            let current = stored.unwrap_or_else(Settings::first_run);
            shortcuts::apply(&handle);
            app.manage(media_duck::MediaDuck::start(
                current.media_duck,
                current.media_duck_level,
                data::desktop_dir(&handle).ok(),
            ));
            // Before the room opens, so its bridge offers `host.updater`.
            updater::setup(&handle);
            open_room(&handle)?;
            if call_card_disabled() {
                debug("call-controls not created: SIDEVOICE_NO_CALL_CARD=1 (#117)");
            } else {
                call_controls::create(&handle)?;
            }
            // The room to the front, last so no window created after it takes it. Setup runs on Ready, after the
            // launch's own activation, which came before any window existed: the window stayed behind (Dock only).
            show_room(&handle);
            application::activate(&handle);
            // How macOS sees the app once everything exists, and again once activation has settled (#117).
            debug(&format!("ready {}", app_report(&handle)));
            tracing::info!("app ready");
            let later = handle.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(5));
                let report = later.clone();
                let _ = later.run_on_main_thread(move || {
                    debug(&format!("settled {}", app_report(&report)));
                    tracing::info!("app settled");
                });
            });
            data::finalize_relaunched_reset();
            reset_handoff::finish_startup(startup);
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == call_controls::LABEL {
                if let WindowEvent::ScaleFactorChanged { .. } = event {
                    call_controls::scale_changed(window.app_handle());
                }
                return;
            }
            if !window.label().starts_with(ROOM_PREFIX) {
                return;
            }
            // Closing the room window hides it: the call and the tray stay alive. Quit from the tray or ⌘Q.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                call_controls::update(window.app_handle());
            }
        })
        .build(context)
        .expect("error while building the Sidevoice app")
        .run(|app, event| {
            application::handle_event(app, &event);
            // Quitting, however the app ends (tray, ⌘Q, logout, a reset, an update's restart; `AppHandle::restart` from
            // the updater's thread comes here too), before `exit()` runs the C++ static destructors (#121): the voice
            // calls stop and every model and audio device is released; the call controls card's window goes; the local
            // host's proxy stops and closes every tunnel; an update ready installs (automatic mode). Not when the updater
            // ends the process itself (Windows, starting the installer): its before-exit hook stops the local host then
            // (`updater::before_exit`).
            if let tauri::RunEvent::Exit = event {
                voice::shutdown(app);
                media_duck::release(app);
                call_controls::close(app);
                local_host::shutdown(app);
                if !app.state::<AppState>().resetting.load(Ordering::SeqCst) {
                    updater::quit(app);
                }
            }
            let _ = (app, event);
        });
}

/// The helper is the same signed executable, launched only after a confirmed reset or uninstall.
pub fn run_reset_helper() {
    data::run_reset_helper();
}

/// The fresh-process check of System Audio Recording (`--sidevoice-capture-check`, docs/MEDIA_DUCK.md): prints the system's
/// answer for this process, which has only just started, and returns. Nothing of the app starts.
#[allow(clippy::disallowed_macros, reason = "the capture check's answer, read by the parent process")]
pub fn run_capture_check() {
    println!("{}", platform::media_duck::capture_check().word());
}

/// Output meant for the person, when no log exists yet.
#[allow(clippy::disallowed_macros, reason = "output for the person, before any log exists")]
fn tell_the_person(text: impl std::fmt::Display) {
    eprintln!("{text}");
}

/// A url as the log may show one of ours: scheme, host, port and path; never userinfo, query or fragment.
fn url_path(url: &tauri::Url) -> String {
    let port = url.port().map(|port| format!(":{port}")).unwrap_or_default();
    format!("{}://{}{port}{}", url.scheme(), url.host_str().unwrap_or(""), url.path())
}

#[cfg(test)]
mod url_path_tests {
    use super::url_path;

    #[test]
    fn a_url_loses_userinfo_query_and_fragment() {
        let url = tauri::Url::parse("https://u:canary-pass@x.example:8443/a/b?canary-q=1#frag").unwrap();
        assert_eq!(url_path(&url), "https://x.example:8443/a/b");
        assert_eq!(
            url_path(&tauri::Url::parse("tauri://localhost/probe.html").unwrap()),
            "tauri://localhost/probe.html"
        );
    }
}
