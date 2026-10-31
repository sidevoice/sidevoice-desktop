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
mod local_host;
/// A look at the process's own memory, for CI's measurements of it (#145). Never in a release.
#[cfg(any(test, feature = "probe"))]
mod memory;
mod platform;
#[cfg(feature = "probe")]
mod probe;
mod tray;
mod updater;
mod voice;

use crate::platform::application;
use crate::platform::diagnostics;
use crate::platform::host;
use crate::platform::shortcut_access;
use serde_json::{json, Value};
use sidevoice_app_core::app::{AppHost, Hooks, HostConfig};
use sidevoice_app_tauri::platform::media_controls::{self, CallState, MediaCommand};
use sidevoice_app_tauri::platform::origin::APP_ORIGIN;
use sidevoice_desktop_core::bridge::{self, CallSnapshot, Command};
use sidevoice_desktop_core::build_info::BuildInfo;
use sidevoice_desktop_core::settings::{MuteShortcut, Registration, Settings};
use sidevoice_desktop_core::{media, settings};
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
    /// The mute shortcut as the system took it, from startup and from every change.
    shortcut: Mutex<MuteShortcut>,
    /// Held across a change of the mute shortcut, from keeping it to registering it, so what is kept and what is
    /// registered never differ. Only off the main thread: registering waits for it.
    shortcut_change: Mutex<()>,
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
            shortcut: Mutex::default(),
            shortcut_change: Mutex::default(),
            resetting: std::sync::atomic::AtomicBool::new(false),
            reset_admission: Mutex::default(),
            reset_helper: Mutex::default(),
        }
    }
}

/// The shortcut's status now: VoiceOver can be turned on or off at any time.
fn shortcut_status(state: &AppState) -> MuteShortcut {
    let mut status = state.shortcut.lock().unwrap().clone();
    let accelerator = status.accelerator.as_deref().unwrap_or_default();
    status.voice_over = status.state == Registration::On && shortcut_access::reserved_chord_active(accelerator);
    status
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

/// Changes the settings with `change`, registers the mute shortcut they then hold and tells the card: how the system
/// took it.
fn change_shortcut(app: &AppHandle, change: impl FnOnce(&mut Settings)) -> Result<MuteShortcut, String> {
    let state = app.state::<AppState>();
    let _one_at_a_time = state.shortcut_change.lock().unwrap();
    let changed = change_settings(app, change)?;
    apply_shortcut(app, &changed.mute_shortcut);
    call_controls::update(app);
    Ok(shortcut_status(&state))
}

/// A refusal as the room page reads it (docs/BRIDGE.md → "Refusals").
pub(crate) fn refusal(key: &str, message: impl std::fmt::Display) -> Value {
    json!({ "key": key, "message": message.to_string() })
}

/// `host.muteShortcut`'s one command: `action` `get`, or `set` with `accelerator` (`null`: none). Refuses
/// `shortcut_invalid` for an accelerator that is not a shortcut the system can read.
#[tauri::command]
async fn mute_shortcut(
    app: AppHandle,
    webview: Webview,
    action: String,
    accelerator: Option<String>,
) -> Result<MuteShortcut, Value> {
    room_page(&app, &webview).map_err(|why| refusal("bad_request", why))?;
    match action.as_str() {
        "get" => Ok(shortcut_status(&app.state::<AppState>())),
        "set" => {
            let text = accelerator.unwrap_or_default();
            let accelerator = shortcut_access::checked(&text)
                .ok_or_else(|| refusal("shortcut_invalid", format!("Not a shortcut: {text}")))?;
            change_shortcut(&app, |s| s.mute_shortcut = accelerator).map_err(|e| refusal("internal", e))
        }
        _ => Err(refusal("bad_request", format!("not a mute shortcut action: {action}"))),
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
        return Err("not the room window".into());
    }
    let actual = webview.url().map(|u| settings::url_origin(&u)).ok();
    if actual.as_deref() != Some(APP_ORIGIN) {
        debug(&format!("bridge_state refused: origin {actual:?}"));
        return Err("not the app's own page".into());
    }
    let previous = std::mem::replace(&mut *state.call.lock().unwrap(), snapshot.clone());
    if let Some(changes) = snapshot.changes_since(&previous) {
        log::info!("bridge_state: {changes}");
    }
    // The whole snapshot only at level debug or with SIDEVOICE_DEBUG=1, and without the rest of the room's projection:
    // its facts carry the conversation's words, which stay out of the log.
    debug_detail(|| format!("bridge_state {:?}", CallSnapshot { rest: Default::default(), ..snapshot.clone() }));
    tray::update(&app, &snapshot);
    media_controls::update(&app, &media_state(&snapshot));
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
/// room page's (desktop/core/js/webview-log.js). It is rotated at [`LOG_MAX_BYTES`], keeping [`LOG_KEPT`] files.
pub(crate) const LOG_FILE: &str = "sidevoice-desktop";
const LOG_MAX_BYTES: u128 = 2_000_000;
const LOG_KEPT: usize = 5;

/// Where the app log is (it may not exist yet).
pub(crate) fn app_log_file(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_log_dir().ok().map(|dir| dir.join(format!("{LOG_FILE}.log")))
}

fn log_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    use tauri_plugin_log::{FileOpenStrategy, RotationStrategy, Target, TargetKind};
    tauri_plugin_log::Builder::new()
        .clear_targets()
        .target(Target::new(TargetKind::LogDir { file_name: Some(LOG_FILE.into()) }))
        .level(sidevoice_desktop_core::logging::level(
            std::env::var(sidevoice_desktop_core::logging::LEVEL_VARIABLE).ok().as_deref(),
        ))
        .level_for("tao", log::LevelFilter::Warn)
        .level_for("wry", log::LevelFilter::Warn)
        .max_file_size(LOG_MAX_BYTES)
        .rotation_strategy(RotationStrategy::KeepSome(LOG_KEPT))
        .file_open_strategy(FileOpenStrategy::Rotate)
        .build()
}

/// A panic goes to the log too (then to stderr as before).
fn log_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        previous(info);
    }));
}

// Diagnostics on stderr with `SIDEVOICE_DEBUG=1` (CI's smoke tests read them): sidevoice-app-core's, shared with the
// call host.
#[cfg(feature = "probe")]
use sidevoice_app_core::debug::enabled as debugging;
pub(crate) use sidevoice_app_core::debug::{debug, debug_detail};

/// A line from a page's diagnostics (the CI probe), printed only with `SIDEVOICE_DEBUG=1`.
#[tauri::command]
fn debug_log(app: AppHandle, line: String) {
    debug(&format!("page says: {}", line.chars().take(2000).collect::<String>()));
    #[cfg(feature = "probe")]
    probe::exit_on(&app, &line);
    #[cfg(feature = "probe")]
    probe::memory_on(&app, &line);
    let _ = (app, line);
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

/// What the headset buttons and Now Playing need of the call the room reported.
fn media_state(snapshot: &CallSnapshot) -> CallState {
    CallState {
        ready: snapshot.ready,
        joined: snapshot.joined,
        mic_enabled: snapshot.mic_enabled,
        title: snapshot.title.clone(),
    }
}

fn reset_call_state(app: &AppHandle) {
    let snapshot = CallSnapshot::default();
    *app.state::<AppState>().call.lock().unwrap() = snapshot.clone();
    tray::update(app, &snapshot);
    media_controls::update(app, &media_state(&snapshot));
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
        .initialization_script(bridge::NO_CONTEXT_MENU_SCRIPT)
        .initialization_script(bridge::WEBVIEW_LOG_SCRIPT)
        .initialization_script(bridge::script_for_origin(
            APP_ORIGIN,
            host::updates_available(&build.channel),
            &build,
            host::name().as_deref(),
            &debug_options,
            sidevoice_app_tauri::platform::microphone::settings_available(),
            &diagnostics::webview_switches(),
            media_controls::available(),
            platform::uninstall::available(),
            host::system(),
            &bridge::Parts { transport: sidevoice_app_tauri::js::TRANSPORT, members: sidevoice_app_core::js::MEMBERS },
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
            PermissionResponse::Deny
        })
        .on_page_load(move |_webview, payload| {
            // `Started` is WebKit's didCommitNavigation: after redirects, main frame only.
            if payload.event() == PageLoadEvent::Started {
                debug(&format!("page {}", payload.url()));
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

/// Replace the shortcut, keep the actual outcome, and notify the room page.
fn apply_shortcut(app: &AppHandle, accelerator: &str) {
    let status = shortcut_access::replace(app, accelerator);
    let app_state = app.state::<AppState>();
    *app_state.shortcut.lock().unwrap() = status;
    if let Some(window) = room_window(app) {
        let status = json!(shortcut_status(&app_state));
        let _ = window.eval(format!("window.__sidevoiceHost && window.__sidevoiceHost.muteShortcutEvent({status})"));
    }
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
        local_host::local_host_action,
        local_host::local_host_pairing_code,
        local_host::local_host_install,
        local_host::local_host_install_progress,
        local_host::local_host_install_cancel,
        data::data_delete_all,
        data::data_uninstall,
        data::data_reset_report,
        mute_shortcut,
        call_controls::call_controls_settings,
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
            eprintln!("{}", sidevoice_desktop_core::i18n::t(language, "startup.reset_busy"));
            return;
        }
    };
    let context = tauri::generate_context!();
    #[cfg(feature = "probe")]
    let context = probe::with_page(context);
    #[cfg(feature = "probe")]
    probe::watch_exit_handlers();
    log_panics();
    // First, so everything after it is logged.
    // Where the call's events go: over the channel the room page opens (the shared plugin's `voice_events`).
    let pages = sidevoice_app_tauri::Pages::default();
    let events = pages.clone();
    let builder = tauri::Builder::default().plugin(log_plugin());
    // The call controls card is a non-activating panel on macOS (src/call_controls.rs).
    let builder = <platform::card_window::NativeCardWindow as platform::card_window::CardWindow>::configure(builder);
    application::configure(builder)
        .plugin(tauri_plugin_opener::Builder::new().open_js_links_on_click(false).build())
        // The commands the apps share (`host.voice`, `host.engine`): who may call is this app's room page.
        .plugin(sidevoice_app_tauri::init(room_page, pages))
        .manage(AppState::default())
        .invoke_handler(handler())
        .setup(move |app| {
            let handle = app.handle().clone();
            application::prepare(&handle)?;
            shortcut_access::configure(&handle);
            debug(&format!("launch {}", app_report(&handle)));
            tray::create(&handle)?;
            let media_app = handle.clone();
            media_controls::attach(
                &handle,
                Arc::new(move |command| match command {
                    MediaCommand::ToggleMute => send(&media_app, Command::ToggleMute),
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
                },
            ));
            // What the shared plugin's commands hand to the app: the levels to the card, the pairings to the private file.
            app.manage(sidevoice_app_tauri::Observers(Arc::new(app_hooks::CardLevels(handle.clone()))));
            app.manage(sidevoice_app_tauri::Pairings(Arc::new(data::Pairings(handle.clone()))));
            // This computer's own core, on every OS: found, paired, proxied for the page (docs/LOCAL_HOST.md).
            local_host::setup(&handle);
            let stored = data::desktop_dir(&handle).ok().and_then(|dir| settings::load(&dir));
            *app.state::<AppState>().settings.lock().unwrap() = stored.clone();
            // First run too: the interface itself asks for a pairing code; nothing has to be set up first.
            let current = stored.unwrap_or_else(Settings::first_run);
            apply_shortcut(&handle, &current.mute_shortcut);
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
            let later = handle.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(5));
                let report = later.clone();
                let _ = later.run_on_main_thread(move || debug(&format!("settled {}", app_report(&report))));
            });
            data::finalize_relaunched_reset();
            reset_handoff::finish_startup(startup);
            Ok(())
        })
        .on_window_event(|window, event| {
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
