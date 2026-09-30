//! Sidevoice desktop: a Tauri shell whose main window loads the configured room (or node) and
//! whose tray drives the call through the web UI's own actions (docs/BRIDGE.md).
//!
//! Windows:
//! - `room-N`: the room page, remote. Created per configured URL (N grows), so the bridge script
//!   injected into it is bound to exactly one origin. Closing it hides it; the app keeps running.
//! - `settings`: bundled local page (`ui/`), first run and "Ajustes…".

mod tray;

use serde::Serialize;
use sidevoice_desktop_core::bridge::{self, CallSnapshot, Command};
use sidevoice_desktop_core::settings::Settings;
use sidevoice_desktop_core::{media, settings};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tauri::ipc::CapabilityBuilder;
use tauri::utils::config::BackgroundThrottlingPolicy;
use tauri::webview::{PageLoadEvent, PermissionKind, PermissionResponse};
use tauri::{AppHandle, Manager, State, Webview, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

const SETTINGS_LABEL: &str = "settings";
const ROOM_PREFIX: &str = "room-";

/// Safari's own user agent. WKWebView's default one lacks the `Version/… Safari/…` tokens, and
/// Google sign-in (behind the operator's room, via oauth2-proxy) refuses browsers it takes for an
/// embedded webview. See docs/MACOS.md → "Inicio de sesión".
#[cfg(target_os = "macos")]
const MAC_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";

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
    SettingsInfo {
        first_run: stored.is_none(),
        settings: stored.unwrap_or_default(),
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

    let room_changed = previous.as_ref().map(|p| p.room_url != settings.room_url).unwrap_or(true);
    if room_changed || room_window(&app).is_none() {
        open_room(&app, &settings).map_err(|e| format!("No se pudo abrir la sala: {e}"))?;
    }
    show_room(&app);
    if warning.is_none() {
        if let Some(w) = app.get_webview_window(SETTINGS_LABEL) {
            let _ = w.close();
        }
    }
    Ok(SaveResult { settings, warning })
}

/// Called by the bridge script in the room page. Only the room window, on the configured origin,
/// may call it (runtime capability + the check below).
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
    let expected = state.settings.lock().unwrap().as_ref().and_then(|s| settings::origin_of(&s.room_url).ok());
    let actual = webview.url().map(|u| u.origin().ascii_serialization()).ok();
    if expected.is_none() || expected != actual {
        debug(&format!("bridge_state refused: origin {actual:?}, expected {expected:?}"));
        return Err("not the configured room origin".into());
    }
    debug(&format!("bridge_state {snapshot:?}"));
    *state.call.lock().unwrap() = snapshot.clone();
    tray::update(&app, &snapshot);
    Ok(())
}

/// Diagnostics on stderr, only with `SIDEVOICE_DEBUG=1` (CI's smoke test reads them).
fn debug(message: &str) {
    if std::env::var_os("SIDEVOICE_DEBUG").is_some_and(|v| v == "1") {
        eprintln!("sidevoice: {message}");
    }
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
}

/// (Re)creates the room window for `settings.room_url`, bound to that origin.
fn open_room(app: &AppHandle, settings: &Settings) -> tauri::Result<()> {
    let origin = settings::origin_of(&settings.room_url).map_err(|e| tauri::Error::Anyhow(e.into()))?;
    let pattern = settings::remote_pattern(&settings.room_url).map_err(|e| tauri::Error::Anyhow(e.into()))?;
    let url: url::Url = settings.room_url.parse().map_err(|e: url::ParseError| tauri::Error::Anyhow(e.into()))?;

    let state = app.state::<AppState>();
    let label = format!("{ROOM_PREFIX}{}", state.room_counter.fetch_add(1, Ordering::SeqCst) + 1);

    // The room page may report call state, and nothing else. Bound to this window and origin only.
    app.add_capability(
        CapabilityBuilder::new(format!("room-bridge-{label}"))
            .remote(pattern)
            .window(label.clone())
            .permission("allow-bridge-state"),
    )?;

    if let Some(old) = room_window(app) {
        old.destroy()?;
    }
    *state.room_label.lock().unwrap() = Some(label.clone());
    reset_call_state(app);

    // The origin of the page the room window has committed to, kept from the page-load events so
    // the permission handler never has to query the webview re-entrantly from WebKit's delegate.
    let committed_origin: Arc<Mutex<Option<String>>> = Arc::default();
    let committed_for_load = committed_origin.clone();
    let page_app = app.clone();
    let room_origin = origin.clone();
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .title("Sidevoice")
        .inner_size(1100.0, 760.0)
        .min_inner_size(420.0, 560.0)
        .initialization_script(bridge::script_for_origin(&origin))
        // A hidden window still carries the call: never suspend or throttle its page.
        .background_throttling(BackgroundThrottlingPolicy::Disabled)
        // The room and its sign-in pages (oauth2-proxy, Google); never local files or app schemes.
        .on_navigation(|url| media::navigation_allowed(url.scheme()))
        .on_permission_request(move |_webview, kind| {
            let capture = match kind {
                PermissionKind::Microphone => media::Capture::Microphone,
                PermissionKind::Camera => media::Capture::Camera,
                _ => media::Capture::Other,
            };
            let page = committed_origin.lock().unwrap().clone();
            let decision = media::decide(capture, page.as_deref(), &room_origin);
            debug(&format!("media {capture:?} for {page:?}: {decision:?}"));
            match decision {
                media::Decision::Allow => PermissionResponse::Allow,
                media::Decision::Deny => PermissionResponse::Deny,
            }
        })
        .on_page_load(move |_webview, payload| {
            // `Started` is WebKit's didCommitNavigation: after redirects, main frame only.
            if payload.event() == PageLoadEvent::Started {
                *committed_for_load.lock().unwrap() = Some(payload.url().origin().ascii_serialization());
                debug(&format!("page {}", payload.url()));
                reset_call_state(&page_app);
            }
        });
    #[cfg(target_os = "macos")]
    let builder = builder.user_agent(MAC_USER_AGENT);
    builder.build()?;
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

fn open_settings(app: &AppHandle) {
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
        .invoke_handler(tauri::generate_handler![get_settings, save_settings, bridge_state])
        .setup(|app| {
            let handle = app.handle().clone();
            tray::create(&handle)?;
            let stored = app.path().app_config_dir().ok().and_then(|dir| settings::load(&dir));
            *app.state::<AppState>().settings.lock().unwrap() = stored.clone();
            match stored {
                Some(s) => {
                    if let Err(e) = apply_shortcut(&handle, &s.mute_shortcut) {
                        eprintln!("sidevoice: {e}");
                    }
                    open_room(&handle, &s)?;
                }
                None => open_settings(&handle),
            }
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
        .build(tauri::generate_context!())
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
