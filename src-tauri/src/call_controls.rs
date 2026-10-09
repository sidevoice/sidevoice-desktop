//! The call controls card (sidevoice/sidevoice-desktop#4): a small window that floats over the person's other apps
//! during a call, while the room's window is not in front, and never takes focus from them. It shows the web build's
//! `call-controls.html`, which talks only to `bridge/call-controls-bridge.js` (docs/BRIDGE.md → "The call controls
//! card"); the app relays the call to it from the room window and carries its buttons back.
//!
//! - macOS: a non-activating `NSPanel` (tauri-nspanel) at the status level, on every Space and over full-screen apps.
//! - Windows / Linux X11: a borderless, transparent, always-on-top window that is never focused (no taskbar button).
//! - Linux Wayland: the compositor decides position and stacking, and the app cannot see the pointer: the card is an
//!   ordinary small window there, moved by the compositor and hovered by its own pointer events.
//!
//! When it shows, where it sits and how it moves are `sidevoice_desktop_core::call_controls`.

use crate::{debug, AppState};
use serde::Deserialize;
use sidevoice_desktop_core::bridge::{self, CallSnapshot, Command};
use sidevoice_desktop_core::call_controls::{self as logic, Display, Placements, Point, Rect, Size};
use sidevoice_desktop_core::settings::{self, APP_ORIGIN};
use std::path::PathBuf;
use std::sync::{Condvar, Mutex};
use std::time::Duration;
use tauri::utils::config::BackgroundThrottlingPolicy;
use tauri::webview::{PermissionKind, PermissionResponse};
use tauri::{AppHandle, LogicalSize, Manager, Monitor, Webview, WebviewUrl, WebviewWindow};

pub const LABEL: &str = "call-controls";
const PAGE: &str = "voice/call-controls.html";
/// The card at rest, its window's shadow margin included, until the page reports its own size.
const FIRST_SIZE: Size = Size { width: 344.0, height: 82.0 };
/// How often the app looks at the pointer while the card shows: a window that never takes focus may get no hover
/// events (WebKit tracks the mouse in the key window only), so the app tells the card when the pointer is over it.
const POINTER_EVERY: Duration = Duration::from_millis(80);
/// The card's shadow margin inside its window (the web build's CARD_MARGIN): the pointer over it is not over the card.
const MARGIN: f64 = 12.0;

#[cfg(target_os = "macos")]
tauri_nspanel::tauri_panel! {
    panel!(CallControlsPanel {
        config: {
            can_become_key_window: false,
            can_become_main_window: false,
            is_floating_panel: true
        }
    })
}

#[derive(Default)]
struct Inner {
    /// Hidden from the tray for the call that is on; the next call shows it again.
    hidden_for_call: bool,
    shown: bool,
    level: u8,
    /// `None` where the app cannot see the pointer (Wayland): the card's own pointer events decide.
    pointer_inside: Option<bool>,
    /// Clicks in other apps while the card shows (macOS, Windows): each closes a panel the card has open.
    outside_clicks: u64,
    /// The window's size now, as the page reports it.
    size: Option<Size>,
    /// The card at rest: the smallest size the page has reported. Where it rests is always worked out from this one,
    /// so a card hidden while grown comes back where it rested.
    rest_size: Option<Size>,
    /// The display the card rests on, and where on it.
    rest: Option<(Display, Point)>,
    drag: Option<Drag>,
    placements: Placements,
    config_dir: Option<PathBuf>,
}

/// A drag in progress: where the pointer and the window were when it started, on the desktop.
#[derive(Clone, Copy)]
struct Drag {
    pointer: Point,
    window: Point,
}

/// The card's state, and the signal its pointer watcher waits on while the card is hidden.
#[derive(Default)]
pub struct Card(Mutex<Inner>, Condvar);

/// Whether the app runs as a Wayland client: GTK picks Wayland when there is a Wayland display, unless `GDK_BACKEND`
/// says otherwise (`GDK_BACKEND=x11` runs it under XWayland, where it is an X11 card).
fn wayland() -> bool {
    cfg!(target_os = "linux")
        && std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty())
        && !std::env::var("GDK_BACKEND").is_ok_and(|b| b.trim_start().starts_with("x11"))
}

/// Creates the card's window, hidden; it shows during a call ([`update`]).
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let config_dir = app.path().app_config_dir().ok();
    let placements = config_dir.as_deref().map(Placements::load).unwrap_or_default();
    let pointer_inside = if wayland() { None } else { Some(false) };
    app.manage(Card(
        Mutex::new(Inner { placements, config_dir, pointer_inside, ..Default::default() }),
        Condvar::new(),
    ));
    #[allow(unused_mut)] // the probe build adds its own
    let mut script = bridge::call_controls_script(APP_ORIGIN);
    #[cfg(feature = "probe")]
    if let Some(probe) = crate::probe::card_script() {
        script.push_str(probe);
    }
    let size = FIRST_SIZE;

    #[cfg(target_os = "macos")]
    {
        use tauri_nspanel::{CollectionBehavior, PanelBuilder, PanelLevel, StyleMask};
        PanelBuilder::<_, CallControlsPanel>::new(app, LABEL)
            .url(WebviewUrl::App(PAGE.into()))
            .size(tauri::Size::Logical(LogicalSize::new(size.width, size.height)))
            .level(PanelLevel::Status)
            .collection_behavior(
                CollectionBehavior::new().can_join_all_spaces().full_screen_auxiliary().stationary().ignores_cycle(),
            )
            .add_style_mask(StyleMask::empty().nonactivating_panel())
            .hides_on_deactivate(false)
            .has_shadow(false)
            .transparent(true)
            .no_activate(true)
            .with_window(move |w| window_options(w, script))
            .build()?;
        prevent_activation(app);
        watch_clicks(app);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let builder = tauri::WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(PAGE.into()))
            .inner_size(size.width, size.height)
            .always_on_top(true)
            .visible_on_all_workspaces(true)
            .skip_taskbar(true)
            .focusable(false)
            .shadow(false);
        window_options(builder, script).build()?;
    }
    if !wayland() {
        watch_pointer(app.clone());
    }
    debug("call-controls created");
    Ok(())
}

/// What every platform's card window has.
fn window_options<'a, R: tauri::Runtime, M: Manager<R>>(
    builder: tauri::WebviewWindowBuilder<'a, R, M>,
    script: String,
) -> tauri::WebviewWindowBuilder<'a, R, M> {
    builder
        .title("Sidevoice")
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .visible(false)
        .focused(false)
        .accept_first_mouse(true)
        .initialization_script(script)
        // It shows a live call: never suspend or throttle it.
        .background_throttling(BackgroundThrottlingPolicy::Disabled)
        .on_navigation(|url| {
            sidevoice_desktop_core::media::navigation_allowed(url.scheme(), &settings::url_origin(url), APP_ORIGIN)
        })
        // The card captures nothing: the microphone is the room's. Without a handler WebKit would grant.
        .on_permission_request(|_webview, kind: PermissionKind| {
            debug(&format!("call-controls permission {kind:?} denied"));
            PermissionResponse::Deny
        })
}

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

fn card(app: &AppHandle) -> Option<tauri::State<'_, Card>> {
    app.try_state::<Card>()
}

/// Whether the room's window is in front and focused (its own controls are there).
fn room_in_front(app: &AppHandle) -> bool {
    crate::room_window(app).is_some_and(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false))
}

/// Shows or hides the card for the call as it is now, and tells it what changed. Called on every change of the call,
/// of the room window's focus, of the settings and of the tray's "hide".
pub fn update(app: &AppHandle) {
    let Some(card) = card(app) else { return };
    let call = app.state::<AppState>().call.lock().unwrap().clone();
    let in_front = room_in_front(app);
    let (show, hide) = {
        let mut inner = card.0.lock().unwrap();
        if !call.joined {
            inner.hidden_for_call = false;
        }
        let wanted = logic::wanted(call.joined, inner.hidden_for_call, in_front);
        let change = (wanted && !inner.shown, !wanted && inner.shown);
        inner.shown = wanted;
        if change.1 {
            // Nothing of this showing carries over to the next: no pointer over it, no level.
            inner.drag = None;
            inner.level = 0;
            if inner.pointer_inside.is_some() {
                inner.pointer_inside = Some(false);
            }
        }
        change
    };
    if show {
        place(app);
        set_visible(app, true);
        debug("call-controls shown");
        card.1.notify_all();
    } else if hide {
        set_visible(app, false);
        debug("call-controls hidden");
    }
    push(app, full_state(app, &call));
}

/// The tray's "Hide / Show call controls": for the call that is on.
pub fn toggle_hidden(app: &AppHandle) {
    if let Some(card) = card(app) {
        let mut inner = card.0.lock().unwrap();
        inner.hidden_for_call = !inner.hidden_for_call;
    }
    update(app);
}

/// Whether the card is hidden for this call (the tray's label says the opposite).
pub fn hidden_for_call(app: &AppHandle) -> bool {
    card(app).is_some_and(|card| card.0.lock().unwrap().hidden_for_call)
}

/// The microphone's level from the room window, for the card's wave.
pub fn level(app: &AppHandle, level: u8) {
    let Some(card) = card(app) else { return };
    let shown = {
        let mut inner = card.0.lock().unwrap();
        inner.level = level;
        inner.shown
    };
    if shown {
        push(app, serde_json::json!({ "level": level }));
    }
}

fn full_state(app: &AppHandle, call: &CallSnapshot) -> serde_json::Value {
    let settings =
        app.state::<AppState>().settings.lock().unwrap().clone().unwrap_or_else(settings::Settings::first_run);
    let (level, pointer, clicks) = card(app)
        .map(|card| {
            let inner = card.0.lock().unwrap();
            (inner.level, inner.pointer_inside, inner.outside_clicks)
        })
        .unwrap_or((0, None, 0));
    serde_json::json!({
        "call": call,
        "level": level,
        "pointerInside": pointer,
        "outsideClicks": clicks,
        "alwaysExpanded": settings.call_controls_always,
        "muteShortcut": logic::shortcut_label(&settings.mute_shortcut, cfg!(target_os = "macos")),
    })
}

fn push(app: &AppHandle, patch: serde_json::Value) {
    if let Some(w) = window(app) {
        let _ = w.eval(bridge::call_controls_push(&patch));
    }
}

fn set_visible(app: &AppHandle, visible: bool) {
    #[cfg(target_os = "macos")]
    {
        use tauri_nspanel::ManagerExt;
        let app = app.clone();
        // AppKit wants its windows touched on the main thread.
        let _ = app.clone().run_on_main_thread(move || {
            if let Ok(panel) = app.get_webview_panel(LABEL) {
                // orderFrontRegardless / orderOut: shown without activating the app or taking the key window.
                if visible {
                    panel.show();
                    debug(&format!("call-controls panel {}", describe(panel.as_panel())));
                } else {
                    panel.hide()
                }
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(w) = window(app) {
        let _ = if visible { w.show() } else { w.hide() };
    }
}

/// What the panel is, for CI's log (SIDEVOICE_DEBUG=1): how it was asked to behave, read back from AppKit.
#[cfg(target_os = "macos")]
fn describe(panel: &tauri_nspanel::objc2_app_kit::NSPanel) -> String {
    use tauri_nspanel::objc2::msg_send;
    // NSWindowStyleMaskNonactivatingPanel, NSWindowCollectionBehaviorCanJoinAllSpaces / FullScreenAuxiliary.
    const NONACTIVATING: usize = 1 << 7;
    const ALL_SPACES: usize = 1 << 0;
    const FULL_SCREEN_AUXILIARY: usize = 1 << 8;
    let (mask, level, behavior, key, visible, number): (usize, isize, usize, bool, bool, isize) = unsafe {
        (
            msg_send![panel, styleMask],
            msg_send![panel, level],
            msg_send![panel, collectionBehavior],
            msg_send![panel, isKeyWindow],
            msg_send![panel, isVisible],
            msg_send![panel, windowNumber],
        )
    };
    format!(
        "nonactivating={} level={level} all_spaces={} fullscreen_auxiliary={} key={key} visible={visible} window={number} pid={} {}",
        mask & NONACTIVATING != 0,
        behavior & ALL_SPACES != 0,
        behavior & FULL_SCREEN_AUXILIARY != 0,
        std::process::id(),
        app_active()
    )
}

/// Whether the app is the active one: showing, hovering or clicking the card must never make it so (CI's log).
fn app_active() -> String {
    #[cfg(target_os = "macos")]
    {
        use tauri_nspanel::objc2::{class, msg_send, runtime::AnyObject};
        let active: bool = unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            msg_send![app, isActive]
        };
        format!("app_active={active}")
    }
    #[cfg(not(target_os = "macos"))]
    String::new()
}

// Geometry on the desktop (see `Display`): physical pixels on Windows and Linux; points on macOS, where Tao's
// "physical" positions are each scaled by a different display (the pointer's by the main one, a window's by its own).

/// One logical pixel of this window, in desktop units.
fn unit(w: &WebviewWindow) -> f64 {
    if cfg!(target_os = "macos") {
        1.0
    } else {
        w.scale_factor().unwrap_or(1.0)
    }
}

/// The pointer on the desktop.
fn pointer(app: &AppHandle) -> Option<Point> {
    let p = app.cursor_position().ok()?;
    let scale = if cfg!(target_os = "macos") { app.primary_monitor().ok()??.scale_factor() } else { 1.0 };
    Some(Point { x: p.x / scale, y: p.y / scale })
}

/// The window on the desktop.
fn frame(w: &WebviewWindow) -> Option<Rect> {
    let (at, size) = (w.outer_position().ok()?, w.outer_size().ok()?);
    let scale = if cfg!(target_os = "macos") { w.scale_factor().ok()? } else { 1.0 };
    Some(Rect {
        x: at.x as f64 / scale,
        y: at.y as f64 / scale,
        width: size.width as f64 / scale,
        height: size.height as f64 / scale,
    })
}

fn move_to(w: &WebviewWindow, at: Point) {
    #[cfg(target_os = "macos")]
    let _ = w.set_position(tauri::LogicalPosition::new(at.x, at.y));
    #[cfg(not(target_os = "macos"))]
    let _ = w.set_position(tauri::PhysicalPosition::new(at.x.round() as i32, at.y.round() as i32));
}

/// A monitor's whole area on the desktop, and the display it is to the card (its work area).
fn monitor_display(m: &Monitor) -> (Rect, Display) {
    let scale = m.scale_factor();
    let (unit, to_desktop) = if cfg!(target_os = "macos") { (1.0, 1.0 / scale) } else { (scale, 1.0) };
    let (at, size, work) = (m.position(), m.size(), m.work_area());
    let whole = Rect {
        x: at.x as f64 * to_desktop,
        y: at.y as f64 * to_desktop,
        width: size.width as f64 * to_desktop,
        height: size.height as f64 * to_desktop,
    };
    let name = m.name().cloned().unwrap_or_else(|| format!("{}x{}", size.width, size.height));
    let display = Display::new(
        &name,
        unit,
        Point { x: work.position.x as f64 * to_desktop, y: work.position.y as f64 * to_desktop },
        Size { width: work.size.width as f64 / scale, height: work.size.height as f64 / scale },
    );
    (whole, display)
}

/// The display with this key, else the one at `at` (on the desktop), else the room window's, else the primary.
fn display(app: &AppHandle, key: Option<&str>, at: Option<Point>) -> Option<Display> {
    let all: Vec<(Rect, Display)> = app.available_monitors().unwrap_or_default().iter().map(monitor_display).collect();
    let by_key = key.and_then(|key| all.iter().find(|(_, d)| d.key == key));
    let by_point = || at.and_then(|p| all.iter().find(|(whole, _)| whole.contains(p)));
    if let Some((_, d)) = by_key.or_else(by_point) {
        return Some(d.clone());
    }
    crate::room_window(app)
        .and_then(|w| w.current_monitor().ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten())
        .map(|m| monitor_display(&m).1)
}

/// Puts the card where it rests on its display (where it was left there, or the top-right corner the first time),
/// raised just enough for its current size to fit. On Wayland the compositor places it.
fn place(app: &AppHandle) {
    if wayland() {
        return;
    }
    let (Some(card), Some(w)) = (card(app), window(app)) else { return };
    let mut inner = card.0.lock().unwrap();
    let rest_size = inner.rest_size.unwrap_or(FIRST_SIZE);
    let size = inner.size.unwrap_or(rest_size);
    let Some(display) = display(app, inner.rest.as_ref().map(|(d, _)| d.key.as_str()), None) else { return };
    let stored = match &inner.rest {
        Some((d, p)) if d.key == display.key => Some(*p),
        _ => inner.placements.displays.get(&display.key).copied(),
    };
    let rest = logic::resting_place(stored, rest_size, display.work);
    let at = logic::fit(rest, size, display.work);
    move_to(&w, display.to_desktop(at));
    debug(&format!(
        "call-controls at {:.0},{:.0} on {} size {:.0}x{:.0} rest {:.0},{:.0}",
        at.x, at.y, display.key, size.width, size.height, rest.x, rest.y
    ));
    inner.rest = Some((display, rest));
}

fn from_card_window(webview: &Webview) -> Result<(), String> {
    if webview.label() != LABEL {
        return Err("not the call controls window".into());
    }
    let actual = webview.url().map(|u| settings::url_origin(&u)).ok();
    if actual.as_deref() != Some(APP_ORIGIN) {
        return Err("not the app's own page".into());
    }
    Ok(())
}

/// The card's page is ready: it gets the whole state.
#[tauri::command]
pub fn call_controls_ready(app: AppHandle, webview: Webview) -> Result<(), String> {
    from_card_window(&webview)?;
    let call = app.state::<AppState>().call.lock().unwrap().clone();
    push(&app, full_state(&app, &call));
    Ok(())
}

/// One of the card's commands: "open-app" is the app's, the rest are carried to the room page.
#[tauri::command]
pub fn call_controls_run(app: AppHandle, webview: Webview, command: serde_json::Value) -> Result<(), String> {
    from_card_window(&webview)?;
    if command.get("command").and_then(|c| c.as_str()) == Some("open-app") {
        crate::show_room(&app);
        return Ok(());
    }
    let command: Command = serde_json::from_value(command.clone()).map_err(|e| {
        debug(&format!("call-controls refused {command}: {e}"));
        format!("not a card command: {e}")
    })?;
    debug(&format!("call-controls run {} {}", command.name(), app_active()));
    crate::send(&app, command);
    Ok(())
}

/// The card's size with its margin: the window takes it, raised as needed to fit its display.
#[tauri::command]
pub fn call_controls_layout(app: AppHandle, webview: Webview, width: f64, height: f64) -> Result<(), String> {
    from_card_window(&webview)?;
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 && width < 2000.0 && height < 2000.0) {
        return Err("not a card size".into());
    }
    let size = Size { width, height };
    if let Some(card) = card(&app) {
        let mut inner = card.0.lock().unwrap();
        inner.size = Some(size);
        if inner.rest_size.is_none_or(|rest| height < rest.height) {
            inner.rest_size = Some(size);
        }
    }
    if let Some(w) = window(&app) {
        let _ = w.set_size(LogicalSize::new(width, height));
    }
    place(&app);
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DragPhase {
    Start,
    Move,
    End,
}

/// Moving the card: the window follows the pointer; dropped, it snaps (free with magnet) and is remembered for that
/// display. The app follows the pointer itself, on the desktop, so displays of different scales agree. On Wayland the
/// compositor moves the window, and where it is left is not the app's to know.
#[tauri::command]
pub fn call_controls_drag(app: AppHandle, webview: Webview, phase: DragPhase) -> Result<(), String> {
    from_card_window(&webview)?;
    let (Some(card), Some(w)) = (card(&app), window(&app)) else { return Ok(()) };
    if wayland() {
        if let DragPhase::Start = phase {
            let _ = w.start_dragging();
        }
        return Ok(());
    }
    let mut inner = card.0.lock().unwrap();
    match phase {
        DragPhase::Start => {
            inner.drag =
                pointer(&app).zip(frame(&w)).map(|(pointer, at)| Drag { pointer, window: Point { x: at.x, y: at.y } });
        }
        DragPhase::Move => {
            if let (Some(drag), Some(p)) = (inner.drag, pointer(&app)) {
                move_to(&w, Point { x: drag.window.x + p.x - drag.pointer.x, y: drag.window.y + p.y - drag.pointer.y });
            }
        }
        DragPhase::End => {
            let (Some(drag), Some(p)) = (inner.drag.take(), pointer(&app)) else { return Ok(()) };
            let dropped = Point { x: drag.window.x + p.x - drag.pointer.x, y: drag.window.y + p.y - drag.pointer.y };
            // The display the pointer let go on; the card's corner may be just off it.
            let Some(display) = display(&app, None, Some(p)) else { return Ok(()) };
            let rest_size = inner.rest_size.or(inner.size).unwrap_or(FIRST_SIZE);
            let rest = logic::drop_at(display.to_local(dropped), rest_size, display.work);
            inner.placements.displays.insert(display.key.clone(), rest);
            if let Some(dir) = &inner.config_dir {
                if let Err(e) = inner.placements.save(dir) {
                    debug(&format!("call-controls placement not saved: {e}"));
                }
            }
            let at = logic::fit(rest, inner.size.unwrap_or(rest_size), display.work);
            move_to(&w, display.to_desktop(at));
            debug(&format!("call-controls dropped at {:.0},{:.0} on {} {}", rest.x, rest.y, display.key, app_active()));
            inner.rest = Some((display, rest));
        }
    }
    Ok(())
}

/// A click in another app while the card shows: the card closes an open panel. (Linux: not detected.)
#[cfg(any(target_os = "macos", windows))]
fn outside_click(app: &AppHandle) {
    let Some(card) = card(app) else { return };
    let clicks = {
        // Only clicks outside the card get here: AppKit's global monitor never sees the app's own windows, and on
        // Windows the pointer thread checks. (The polled pointer may still say "inside" right after it left.)
        let mut inner = card.0.lock().unwrap();
        if !inner.shown {
            return;
        }
        inner.outside_clicks += 1;
        inner.outside_clicks
    };
    debug("call-controls outside click");
    push(app, serde_json::json!({ "outsideClicks": clicks }));
}

/// macOS: AppKit tells the app about clicks in other apps (a global monitor sees mouse buttons without the
/// Accessibility permission; it never sees the keyboard, nor clicks on the app's own windows).
#[cfg(target_os = "macos")]
fn watch_clicks(app: &AppHandle) {
    use tauri_nspanel::objc2::{class, msg_send, runtime::AnyObject};
    // NSEventMaskLeftMouseDown | NSEventMaskRightMouseDown | NSEventMaskOtherMouseDown.
    const MOUSE_DOWN: u64 = (1 << 1) | (1 << 3) | (1 << 25);
    let app = app.clone();
    let handler = block2::RcBlock::new(move |_event: std::ptr::NonNull<AnyObject>| outside_click(&app));
    let monitor: Option<tauri_nspanel::objc2::rc::Retained<AnyObject>> =
        unsafe { msg_send![class!(NSEvent), addGlobalMonitorForEventsMatchingMask: MOUSE_DOWN, handler: &*handler] };
    debug(&format!("call-controls watching clicks elsewhere: {}", monitor.is_some()));
    // For the app's lifetime: the monitor is never removed, and AppKit hands it back autoreleased, so it is kept here.
    std::mem::forget(monitor);
    std::mem::forget(handler);
}

/// macOS: tells the window server, too, that the card never activates the app. NSPanel does so only when it is created
/// with the non-activating style; tauri-nspanel adds that style to an existing window, so AppKit treats the panel as
/// non-activating while the window server does not, and a drag on the card made the app frontmost (focus taken from
/// the person's app, CI's card step). `_setPreventsActivation:` is what NSPanel itself calls at creation:
/// https://philz.blog/nspanel-nonactivating-style-mask-flag/
#[cfg(target_os = "macos")]
fn prevent_activation(app: &AppHandle) {
    use tauri_nspanel::objc2::{msg_send, runtime::Bool, sel};
    use tauri_nspanel::ManagerExt;
    let Ok(panel) = app.get_webview_panel(LABEL) else { return };
    let panel = panel.as_panel();
    // SAFETY: main thread (setup); a private NSWindow method, called only when the window answers to it.
    let applied = unsafe {
        let responds: bool = msg_send![panel, respondsToSelector: sel!(_setPreventsActivation:)];
        if responds {
            let _: () = msg_send![panel, _setPreventsActivation: Bool::YES];
        }
        responds
    };
    debug(&format!("call-controls prevents activation: {applied}"));
}

/// Windows: whether a mouse button went down since the last look (the pointer watcher's pace).
#[cfg(windows)]
fn button_down() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON};
    // The high bit is "down now"; the low bit "pressed since the last call", so a quick click is not missed.
    [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].iter().any(|&key| unsafe { GetAsyncKeyState(key as i32) } as u16 & 0x8001 != 0)
}

/// While the card shows, tells it when the pointer comes over it or leaves (see [`POINTER_EVERY`]); on Windows, also
/// when the person clicks elsewhere. One thread for the app's lifetime, asleep while the card is hidden.
fn watch_pointer(app: AppHandle) {
    std::thread::spawn(move || {
        #[cfg(windows)]
        let mut was_down = false;
        loop {
            {
                let Some(card) = card(&app) else { return };
                let mut inner = card.0.lock().unwrap();
                while !inner.shown {
                    inner = card.1.wait(inner).unwrap();
                }
            }
            std::thread::sleep(POINTER_EVERY);
            let Some(w) = window(&app) else { return };
            let inside = pointer(&app).zip(frame(&w)).map(|(p, at)| {
                let margin = MARGIN * unit(&w);
                Rect {
                    x: at.x + margin,
                    y: at.y + margin,
                    width: at.width - 2.0 * margin,
                    height: at.height - 2.0 * margin,
                }
                .contains(p)
            });
            let changed = {
                let Some(card) = card(&app) else { return };
                let mut inner = card.0.lock().unwrap();
                // Hidden meanwhile: what the pointer did is of no use to the next showing.
                let changed = inner.shown && inner.pointer_inside != inside;
                if changed {
                    inner.pointer_inside = inside;
                }
                changed
            };
            if changed {
                debug(&format!("call-controls pointer inside={inside:?} {}", app_active()));
                push(&app, serde_json::json!({ "pointerInside": inside }));
            }
            #[cfg(windows)]
            {
                let down = button_down();
                if down && !was_down && inside == Some(false) {
                    outside_click(&app);
                }
                was_down = down;
            }
        }
    });
}
