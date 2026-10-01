//! Sidevoice desktop: a Tauri shell around the Sidevoice web interface, bundled in the app, whose tray drives
//! the call through the interface's own actions (docs/BRIDGE.md).
//!
//! The interface is a secret-free client: it talks to a node or a rendezvous room it was paired with (the node
//! issues a one-time code; sidevoice-core `control/devices.py`), told where to look first through
//! `window.__SIDEVOICE_TARGET__` (docs/TARGETS.md). No remote page is ever loaded.
//!
//! Windows:
//! - `room-N`: the bundled interface. Created again when the target changes (N grows), so the scripts injected
//!   into it carry the current target. Closing it hides it; the app keeps running.
//! - `settings`: bundled local page (`ui/index.html`): target, shortcut, diagnostics.

mod engine_ipc;
mod headset;
#[cfg(feature = "probe")]
mod probe;
mod tray;

use serde::Serialize;
use sidevoice_desktop_core::bridge::{self, CallSnapshot, Command};
use sidevoice_desktop_core::settings::{Settings, APP_ORIGIN};
use sidevoice_desktop_core::{media, settings};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tauri::utils::config::BackgroundThrottlingPolicy;
use tauri::webview::{PageLoadEvent, PermissionKind, PermissionResponse};
use tauri::{AppHandle, Manager, State, Webview, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

const SETTINGS_LABEL: &str = "settings";
const ROOM_PREFIX: &str = "room-";
/// The web interface bundled in the app (vendored from sidevoice-web, `scripts/vendor-web.mjs`), at the
/// paths a room serves it from, so its absolute `/voice/…` and `/voice-browser/…` URLs resolve.
const BUNDLED_INTERFACE: &str = "voice/index.html";

#[derive(Default)]
struct AppState {
    settings: Mutex<Option<Settings>>,
    call: Mutex<CallSnapshot>,
    room_label: Mutex<Option<String>>,
    room_counter: AtomicU32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsInfo {
    settings: Settings,
    first_run: bool,
    /// Diagnostics on (`SIDEVOICE_DEBUG=1`): the settings page then also reports what its webview offers.
    debug: bool,
    app_version: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveResult {
    settings: Settings,
    warning: Option<String>,
}

#[tauri::command]
fn get_settings(app: AppHandle, state: State<'_, AppState>) -> SettingsInfo {
    let stored = state.settings.lock().unwrap().clone();
    debug(&format!("get_settings (first run: {})", stored.is_none()));
    SettingsInfo {
        first_run: stored.is_none(),
        settings: stored.unwrap_or_else(Settings::first_run),
        debug: debugging(),
        app_version: app.package_info().version.to_string(),
    }
}

// Async so it runs off the main thread: creating a window from a synchronous command deadlocks
// on Windows (WebView2), and the room window is (re)created here.
#[tauri::command]
async fn save_settings(app: AppHandle, state: State<'_, AppState>, settings: Settings) -> Result<SaveResult, String> {
    let settings = settings::validate(settings).map_err(|e| e.to_string())?;
    if !settings.mute_shortcut.is_empty() && settings.mute_shortcut.parse::<Shortcut>().is_err() {
        return Err(format!("El atajo «{}» no es válido. Ejemplo: CmdOrCtrl+Shift+M.", settings.mute_shortcut));
    }
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    settings::save(&dir, &settings).map_err(|e| format!("No se pudo guardar: {e}"))?;

    let previous = state.settings.lock().unwrap().replace(settings.clone());
    let warning = apply_shortcut(&app, &settings.mute_shortcut).err();

    let target_changed = previous.as_ref().map(|p| p.target != settings.target).unwrap_or(true);
    if target_changed || room_window(&app).is_none() {
        open_room(&app, &settings).map_err(|e| format!("No se pudo abrir la ventana: {e}"))?;
    }
    show_room(&app);
    if warning.is_none() {
        if let Some(w) = app.get_webview_window(SETTINGS_LABEL) {
            let _ = w.close();
        }
    }
    Ok(SaveResult { settings, warning })
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
    engine_ipc::call_changed(&app, snapshot.joined);
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

fn room_window(app: &AppHandle) -> Option<WebviewWindow> {
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
    engine_ipc::call_changed(app, snapshot.joined);
}

/// (Re)creates the room window: the bundled interface, told its target, bound to the app's own pages.
fn open_room(app: &AppHandle, settings: &Settings) -> tauri::Result<()> {
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
    let mut builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(page.into()))
        .title("Sidevoice")
        .inner_size(1100.0, 760.0)
        .min_inner_size(420.0, 560.0)
        .initialization_script(bridge::script_for_origin(APP_ORIGIN));
    if let Some(target) = settings::target_script(settings) {
        builder = builder.initialization_script(target);
    }
    // CI's probe build only (src/probe.rs): the native flow driven through the vendored room itself.
    #[cfg(feature = "probe")]
    if let Some(flow) = probe::room_flow() {
        builder = builder.initialization_script(flow);
    }
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
            let decision = media::decide(capture, page.as_deref(), APP_ORIGIN);
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

fn show_room(app: &AppHandle) {
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

/// Registers the global mute shortcut, replacing any previous one. Errors are for the person.
fn apply_shortcut(app: &AppHandle, accelerator: &str) -> Result<(), String> {
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();
    if accelerator.is_empty() {
        return Ok(());
    }
    shortcuts.register(accelerator).map_err(|e| {
        format!("Guardado, pero el atajo «{accelerator}» no se pudo activar (¿lo usa otra aplicación?): {e}")
    })
}

pub fn run() {
    let context = tauri::generate_context!();
    #[cfg(feature = "probe")]
    let context = probe::with_page(context);
    tauri::Builder::default()
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
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            bridge_state,
            debug_log,
            headset_report,
            headset_test,
            engine_ipc::engine_capabilities,
            engine_ipc::engine_installed,
            engine_ipc::engine_on_disk,
            engine_ipc::engine_install,
            engine_ipc::engine_progress,
            engine_ipc::engine_cancel,
            engine_ipc::engine_transcribe,
            engine_ipc::engine_synthesize,
            engine_ipc::engine_load,
            engine_ipc::engine_unload,
            engine_ipc::engine_loaded,
            engine_ipc::engine_memory
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            tray::create(&handle)?;
            headset::setup(&handle);
            // Native engines and models are downloaded into the app's data directory, on demand (docs/ENGINES.md).
            let engines_root = app.path().app_data_dir()?.join("engines");
            #[allow(unused_mut)] // the probe build may shorten the idle time
            let mut engines = sidevoice_desktop_engine::NativeEngines::new(
                sidevoice_desktop_core::engines::bundled_catalog(),
                engines_root,
            );
            #[cfg(feature = "probe")]
            if let Some(idle) = probe::idle_unload() {
                engines.idle_unload = idle;
            }
            #[cfg(feature = "probe")]
            {
                engines.faults = probe::faults();
            }
            let engines = engine_ipc::EngineState::new(engines);
            // A model stays in memory during a call and for ten minutes after the last use (sidevoice-core#21 D13).
            engine_ipc::unload_when_idle(engines.engines.clone());
            app.manage(engines);
            let stored = app.path().app_config_dir().ok().and_then(|dir| settings::load(&dir));
            *app.state::<AppState>().settings.lock().unwrap() = stored.clone();
            // First run too: the interface itself asks for a pairing code; nothing has to be set up first.
            let current = stored.unwrap_or_else(Settings::first_run);
            if let Err(e) = apply_shortcut(&handle, &current.mute_shortcut) {
                eprintln!("sidevoice: {e}");
            }
            open_room(&handle, &current)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the room window hides it: the call and the tray stay alive. Quit from the tray or ⌘Q.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label().starts_with(ROOM_PREFIX) {
                    api.prevent_close();
                    let _ = window.hide();
                }
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
            let _ = (app, event);
        });
}
