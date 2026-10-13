//! Sidevoice desktop: a Tauri shell around the Sidevoice web interface, bundled in the app, whose tray drives
//! the call through the interface's own actions (docs/BRIDGE.md).
//!
//! The interface is a secret-free client: it talks to a node or a rendezvous room it was paired with (the node
//! issues a one-time code; sidevoice-core `control/devices.py`), and the app never tells it where to look
//! (docs/TARGETS.md). No remote page is ever loaded.
//!
//! Windows:
//! - `room-N`: the bundled interface. Created at launch, and again (N grows) when it is gone. Closing it hides it; the
//!   app keeps running.
//! - `settings`: bundled local page (`ui/index.html`): diagnostics only (native engine, headset buttons, this computer);
//!   the mute shortcut and the call controls are set in the room page's Settings.
//! - `call-controls`: the call controls card (`src/call_controls.rs`), floating over other apps during a call.

mod call_controls;
mod engine_ipc;
mod headset;
mod keychain;
mod local_host;
#[cfg(feature = "probe")]
mod probe;
mod tray;
mod updater;
mod voice;

use serde::Serialize;
use serde_json::{json, Value};
use sidevoice_desktop_core::bridge::{self, CallSnapshot, Command};
use sidevoice_desktop_core::build_info::BuildInfo;
use sidevoice_desktop_core::settings::{MuteShortcut, Registration, Settings, APP_ORIGIN};
use sidevoice_desktop_core::{media, settings};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tauri::utils::config::BackgroundThrottlingPolicy;
use tauri::webview::{PageLoadEvent, PermissionKind, PermissionResponse};
use tauri::{AppHandle, Manager, State, Webview, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

const SETTINGS_LABEL: &str = "settings";
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
}

impl Default for AppState {
    fn default() -> Self {
        let preferred: Vec<String> = sys_locale::get_locales().collect();
        AppState {
            settings: Mutex::default(),
            call: Mutex::default(),
            room_label: Mutex::default(),
            room_counter: AtomicU32::default(),
            language: sidevoice_desktop_core::i18n::language(&preferred),
            shortcut: Mutex::default(),
            shortcut_change: Mutex::default(),
        }
    }
}

/// The shortcut's status now: VoiceOver can be turned on or off at any time.
fn shortcut_status(state: &AppState) -> MuteShortcut {
    let mut status = state.shortcut.lock().unwrap().clone();
    let accelerator = status.accelerator.as_deref().unwrap_or_default();
    status.voice_over =
        status.state == Registration::On && settings::uses_voice_over_keys(accelerator) && voice_over_on();
    status
}

/// Whether VoiceOver is running (macOS 10.13+).
fn voice_over_on() -> bool {
    #[cfg(target_os = "macos")]
    {
        use objc2::{class, msg_send, runtime::AnyObject};
        unsafe {
            let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
            msg_send![workspace, isVoiceOverEnabled]
        }
    }
    #[cfg(not(target_os = "macos"))]
    false
}

/// A mute shortcut as the app keeps it (core `normalize_shortcut`, empty for none), if the system can read its key.
fn checked_shortcut(text: &str) -> Option<String> {
    settings::normalize_shortcut(text).filter(|a| a.is_empty() || a.parse::<Shortcut>().is_ok())
}

/// The one writer of settings.json: `change` applied to the stored settings, saved and kept under one lock, so the
/// room page's setters never lose each other's fields.
pub(crate) fn change_settings(app: &AppHandle, change: impl FnOnce(&mut Settings)) -> Result<Settings, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsInfo {
    /// Diagnostics on (`SIDEVOICE_DEBUG=1`): the settings page then also reports what its webview offers.
    debug: bool,
    app_version: String,
}

/// What the settings window's diagnostics read from the app.
#[tauri::command]
fn get_settings(app: AppHandle) -> SettingsInfo {
    SettingsInfo { debug: debugging(), app_version: app.package_info().version.to_string() }
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
            let accelerator = checked_shortcut(&text)
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
    debug(&format!("bridge_state {snapshot:?}"));
    *state.call.lock().unwrap() = snapshot.clone();
    tray::update(&app, &snapshot);
    headset::update(&app, &snapshot);
    call_controls::update(&app);
    Ok(())
}

/// The microphone's level during a call (the bridge script, at most every 80 ms), for the call controls card. Same
/// caller rule as `bridge_state`.
#[tauri::command]
fn bridge_level(app: AppHandle, webview: Webview, level: f64) -> Result<(), String> {
    room_page(&app, &webview)?;
    call_controls::level(&app, level.clamp(0.0, 100.0).round() as u8);
    Ok(())
}

/// Whether `webview` is the current room window showing the app's own page: the only caller the room's commands
/// accept (`bridge_level`, `local_host_*`; capabilities/room.json grants them to `room-*`, this narrows to the current
/// one and its origin).
pub(crate) fn room_page(app: &AppHandle, webview: &Webview) -> Result<(), String> {
    let current_label = app.state::<AppState>().room_label.lock().unwrap().clone();
    if current_label.as_deref() != Some(webview.label()) {
        return Err("not the room window".into());
    }
    if webview.url().map(|u| settings::url_origin(&u)).ok().as_deref() != Some(APP_ORIGIN) {
        return Err("not the app's own page".into());
    }
    Ok(())
}

fn debugging() -> bool {
    std::env::var_os("SIDEVOICE_DEBUG").is_some_and(|v| v == "1")
}

/// Diagnostics on stderr, only with `SIDEVOICE_DEBUG=1` (CI's smoke tests read them).
pub(crate) fn debug(message: &str) {
    if debugging() {
        eprintln!("sidevoice: {message}");
    }
}

/// The headset diagnostic in the settings window: the last button events and whether they are being listened for.
#[tauri::command]
fn headset_report(app: AppHandle) -> headset::Report {
    app.state::<headset::Headset>().report()
}

/// "Probar botones": the app listens for a headset's buttons for a minute, without a call.
#[tauri::command]
fn headset_test(app: AppHandle) {
    headset::test(&app, 60);
}

/// A line from a page's diagnostics (the settings page, the CI probe), printed only with `SIDEVOICE_DEBUG=1`.
#[tauri::command]
fn debug_log(line: String) {
    debug(&format!("page says: {}", line.chars().take(2000).collect::<String>()));
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

fn reset_call_state(app: &AppHandle) {
    let snapshot = CallSnapshot::default();
    *app.state::<AppState>().call.lock().unwrap() = snapshot.clone();
    tray::update(app, &snapshot);
    headset::update(app, &snapshot);
    call_controls::update(app);
    #[cfg(target_os = "macos")]
    voice::page_changed(app);
}

/// (Re)creates the room window: the bundled interface, bound to the app's own pages.
fn open_room(app: &AppHandle) -> tauri::Result<()> {
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

    // The origin of the page the window has committed to, kept from the page-load events so the permission
    // handler never has to query the webview re-entrantly from WebKit's delegate.
    let committed_origin: Arc<Mutex<Option<String>>> = Arc::default();
    let committed_for_load = committed_origin.clone();
    let page_app = app.clone();
    // `host.build`, as xtask built this copy (build.rs).
    let version = app.package_info().version.to_string();
    let (commit, date) = (env!("SIDEVOICE_BUILD_COMMIT"), env!("SIDEVOICE_BUILD_DATE"));
    let build = BuildInfo::new(env!("SIDEVOICE_BUILD_CHANNEL"), &version, commit, date);
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(page.into()))
        .title("Sidevoice")
        .inner_size(1100.0, 760.0)
        .min_inner_size(420.0, 560.0)
        .initialization_script(bridge::script_for_origin(APP_ORIGIN, updater::offered(), &build));
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
        .on_permission_request(move |_webview, kind| {
            let capture = match kind {
                PermissionKind::Microphone => media::Capture::Microphone,
                PermissionKind::Camera => media::Capture::Camera,
                _ => media::Capture::Other,
            };
            let page = committed_origin.lock().unwrap().clone();
            let decision = media::decide(capture, page.as_deref(), APP_ORIGIN, !bridge::voice_offered());
            debug(&format!("media {capture:?} for {page:?}: {decision:?}"));
            match decision {
                media::Decision::Allow => PermissionResponse::Allow,
                media::Decision::Deny => PermissionResponse::Deny,
            }
        })
        .on_page_load(move |_webview, payload| {
            // `Started` is WebKit's didCommitNavigation: after redirects, main frame only.
            if payload.event() == PageLoadEvent::Started {
                *committed_for_load.lock().unwrap() = Some(settings::url_origin(payload.url()));
                debug(&format!("page {}", payload.url()));
                reset_call_state(&page_app);
            }
        })
        .build()?;
    Ok(())
}

pub(crate) fn show_room(app: &AppHandle) {
    match room_window(app) {
        Some(w) => {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
        None => open_settings(app),
    }
}

pub(crate) fn open_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(SETTINGS_LABEL) {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, SETTINGS_LABEL, WebviewUrl::App("index.html".into()))
        .title("Sidevoice · Ajustes")
        .inner_size(600.0, 640.0)
        .min_inner_size(420.0, 480.0)
        .build();
}

/// Registers the global mute shortcut, replacing any previous one, keeps how it went and tells the room page
/// (`host.muteShortcut.subscribe`). A refused one comes with the alternatives the system does take.
fn apply_shortcut(app: &AppHandle, accelerator: &str) {
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();
    let (state, alternatives) = if accelerator.is_empty() {
        (Registration::Off, vec![])
    } else if let Err(e) = shortcuts.register(accelerator) {
        debug(&format!("shortcut {accelerator} refused: {e}"));
        let alternatives = settings::shortcut_alternatives(accelerator)
            .filter(|a| shortcuts.register(*a).is_ok() && shortcuts.unregister(*a).is_ok())
            .map(str::to_owned)
            .collect();
        (Registration::Refused, alternatives)
    } else {
        (Registration::On, vec![])
    };
    let app_state = app.state::<AppState>();
    *app_state.shortcut.lock().unwrap() =
        MuteShortcut::new(accelerator, state, alternatives, cfg!(target_os = "macos"));
    if let Some(window) = room_window(app) {
        let status = json!(shortcut_status(&app_state));
        let _ =
            window.eval(format!("window.__sidevoiceDesktop && window.__sidevoiceDesktop.muteShortcutEvent({status})"));
    }
}

/// The app's commands, with `$extra` (each after a comma) after them.
macro_rules! commands {
    ($($extra:tt)*) => {
        tauri::generate_handler![
            get_settings,
            bridge_state,
            bridge_level,
            call_controls::call_controls_ready,
            call_controls::call_controls_run,
            call_controls::call_controls_layout,
            call_controls::call_controls_drag,
            debug_log,
            headset_report,
            headset_test,
            engine_ipc::engine_capabilities,
            engine_ipc::engine_installed,
            engine_ipc::engine_on_disk,
            engine_ipc::engine_install,
            engine_ipc::engine_progress,
            engine_ipc::engine_cancel,
            engine_ipc::engine_uninstall,
            engine_ipc::engine_memory,
            engine_ipc::engine_catalogs,
            engine_ipc::engine_set_credential,
            engine_ipc::engine_has_credential,
            engine_ipc::engine_test_credential,
            engine_ipc::engine_refresh,
            local_host::local_host_state,
            local_host::local_host_pairing,
            local_host::local_host_action,
            local_host::local_host_pairing_code,
            local_host::local_host_pair_room,
            mute_shortcut,
            call_controls::call_controls_settings,
            updater::updater
            $($extra)*
        ]
    };
}

/// Every command, the voice call's where the app runs it (`host.voice`, macOS).
#[cfg(target_os = "macos")]
fn handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    commands![
        , voice::voice_set_settings,
        voice::voice_start,
        voice::voice_stop,
        voice::voice_say,
        voice::voice_cancel_say,
        voice::voice_mute,
        voice::voice_cancel_input,
        voice::voice_audio_devices,
        voice::voice_set_audio_devices
    ]
}

#[cfg(not(target_os = "macos"))]
fn handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    commands![]
}

pub fn run() {
    let context = tauri::generate_context!();
    #[cfg(feature = "probe")]
    let context = probe::with_page(context);
    let builder = tauri::Builder::default();
    // The call controls card is a non-activating panel on macOS (src/call_controls.rs).
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());
    // The voice call the app runs (`host.voice`, src/voice.rs).
    #[cfg(target_os = "macos")]
    let builder = builder.manage(voice::VoiceState::default());
    builder
        // A second launch (Finder, Spotlight) brings the running one forward instead.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| show_room(app)))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        send(app, Command::ToggleMute);
                    }
                })
                .build(),
        )
        .manage(AppState::default())
        .invoke_handler(handler())
        .setup(|app| {
            let handle = app.handle().clone();
            tray::create(&handle)?;
            headset::setup(&handle);
            // sidevoice-engine keeps the models it downloads, on demand, in the app's data directory (docs/ENGINES.md).
            // Without it the app still runs; the page then finds no native engine to use.
            let engine_root = app.path().app_data_dir()?.join("sidevoice-engine");
            // The keys of remote providers come from the keychain (macOS); elsewhere the app keeps none.
            #[cfg(target_os = "macos")]
            let credentials = keychain::Keychain;
            #[cfg(not(target_os = "macos"))]
            let credentials = sidevoice_desktop_engine::sidevoice_engine::NoCredentials;
            match sidevoice_desktop_engine::NativeEngines::new(
                engine_root,
                credentials,
                tauri::async_runtime::handle().inner().clone(),
            ) {
                Ok(engines) => {
                    app.manage(engine_ipc::EngineState::new(engines));
                }
                Err(e) => eprintln!("sidevoice: the native engine did not start: {} ({})", e.key, e.message),
            }
            // This computer's own core, if any (macOS): found, paired, proxied for the page (docs/LOCAL_HOST.md).
            local_host::setup(&handle);
            let stored = app.path().app_config_dir().ok().and_then(|dir| settings::load(&dir));
            *app.state::<AppState>().settings.lock().unwrap() = stored.clone();
            // First run too: the interface itself asks for a pairing code; nothing has to be set up first.
            let current = stored.unwrap_or_else(Settings::first_run);
            apply_shortcut(&handle, &current.mute_shortcut);
            // Before the room opens, so its bridge offers `host.updater`.
            updater::setup(&handle);
            open_room(&handle)?;
            call_controls::create(&handle)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if !window.label().starts_with(ROOM_PREFIX) {
                return;
            }
            match event {
                // Closing the room window hides it: the call and the tray stay alive. Quit from the tray or ⌘Q.
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    let _ = window.hide();
                    call_controls::update(window.app_handle());
                }
                // In front, the room has the call's controls; behind, the card does.
                WindowEvent::Focused(_) => call_controls::update(window.app_handle()),
                _ => {}
            }
        })
        .build(context)
        .expect("error while building the Sidevoice app")
        .run(|app, event| {
            // macOS: clicking the Dock icon with no visible window.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { has_visible_windows: false, .. } = event {
                show_room(app);
            }
            // Quitting: the local host's proxy stops and closes every tunnel; an update ready installs (automatic mode).
            if let tauri::RunEvent::Exit = event {
                local_host::shutdown(app);
                updater::quit(app);
            }
            let _ = (app, event);
        });
}
