//! The call controls card (sidevoice/sidevoice-desktop#4): a small window that floats over the person's other apps
//! during a call, while the room's stage is out of view (#165: the room hidden, minimised, covered or on another
//! Space, or the page on Settings), and never takes focus from them. It shows the web build's `call-controls.html`,
//! which talks only to `bridge/call-controls-bridge.js` (docs/BRIDGE.md → "The call controls card"); the app relays the
//! call to it from the room window and carries its buttons back.
//!
//! - macOS: a non-activating `NSPanel` (tauri-nspanel) at the status level, on every Space and over full-screen apps.
//! - Windows / Linux X11: a borderless, transparent, always-on-top window that is never focused (no taskbar button).
//! - Linux Wayland: the compositor decides position and stacking, and the app cannot see the pointer: the card is an
//!   ordinary small window there, moved by the compositor and hovered by its own pointer events.
//!
//! When it shows, where it sits and how it moves are `sidevoice_desktop_core::call_controls`.

use crate::platform::card_window::{CardSpec, CardWindow, NativeCardWindow};
use crate::platform::desktop_geometry::{DesktopGeometry, NativeGeometry};
use crate::{debug, AppState};
use serde::Deserialize;
use sidevoice_desktop_core::bridge::{self, CallSnapshot, Command};
use sidevoice_desktop_core::call_controls::{
    self as logic, Corner, Display, Edge, Placements, Point, Rect, Size, Spot,
};
use sidevoice_desktop_core::settings::{self, APP_ORIGIN};
use std::path::PathBuf;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, LogicalSize, Manager, Webview, WebviewWindow};

pub const LABEL: &str = "call-controls";
const PAGE: &str = "voice/call-controls.html";
/// The card at rest, its window's shadow margin included, until the page reports its own size: the title card at its
/// Medium size, the default format (the web's `CARD_WIDTHS`).
const FIRST_SIZE: Size = Size { width: 414.0, height: 82.0 };
/// How often the app looks at the pointer while the card shows: a window that never takes focus may get no hover
/// events of its own (Windows, Linux X11), so the app tells the card when the pointer is over it. On macOS the card's
/// WebKit gets them too (the native card window tracking area).
const POINTER_EVERY: Duration = Duration::from_millis(80);
/// How often the app looks at whether the room's stage is in view ([`watch_view`]).
const VIEW_EVERY: Duration = Duration::from_millis(500);
/// The card's shadow margin inside its window (the web build's CARD_MARGIN): the pointer over it is not over the card.
const MARGIN: f64 = 12.0;
/// How long the card shows after the last change in Settings → Floating card (`host.callControls.preview`).
const PREVIEW_FOR: Duration = Duration::from_millis(4000);
/// How long it stays once the person is done with it: the pointer over it, or dragging or resizing it, keeps it showing.
const PREVIEW_AFTER_USE: Duration = Duration::from_millis(2000);
/// How often a preview is looked at again while it shows.
const PREVIEW_EVERY: Duration = Duration::from_millis(200);
/// The largest window the page may ask for, in logical pixels.
const MAX_SIDE: f64 = 8000.0;

#[derive(Default)]
struct Inner {
    /// Hidden for now, by the card's × or the tray's "hide": until the stage comes into view or leaves it again, or the
    /// next call (core `still_hidden`).
    hidden: bool,
    /// Whether the room's stage was in view when last looked at ([`stage_in_view`]).
    stage_in_view: Option<bool>,
    /// What the room's page last said of its stage (`host.callControls.setStageVisible`); `None` until it says (a web
    /// build before #165 never does).
    page: Option<PageStage>,
    shown: bool,
    /// Shown for a moment after a change in Settings, while the room is in front (`preview`).
    preview_until: Option<Instant>,
    /// Whether a thread is looking at the preview (one at a time).
    preview_watched: bool,
    level: u8,
    /// `None` where the app cannot see the pointer (Wayland): the card's own pointer events decide.
    pointer_inside: Option<bool>,
    /// Whether the page says the pointer is over something clickable (its CSS cursor is the hand), as it last reported.
    hand: bool,
    /// Clicks in other apps while the card shows (macOS, Windows): each closes a panel the card has open.
    outside_clicks: u64,
    /// The window's size now, as the page reports it.
    size: Option<Size>,
    /// The box the card is placed by, inside its window (core `anchor`): its body, which stays where it rests while
    /// captions, a list or the avatar's dock grow the window around it.
    anchor: Option<Rect>,
    /// The picture-in-picture's least window width, while the page shows it and lets the person resize it.
    min_width: Option<f64>,
    /// …and its least height.
    min_height: Option<f64>,
    /// The display the card rests on, and where on it.
    rest: Option<(Display, Spot)>,
    /// What the card was last told of its place and its resized size.
    told_place: Option<Corner>,
    told_size: Option<Option<Size>>,
    drag: Option<Drag>,
    resize: Option<Resize>,
    placements: Placements,
    config_dir: Option<PathBuf>,
    /// The latest show or hide decided ([`apply_visible`]), and the latest move ([`move_when_latest`]).
    visibility: logic::Latest,
    moves: logic::Latest,
}

/// The room's stage as its page last told it: whether the page shows it (not on Settings or another screen without it),
/// and where it is in the page (CSS pixels from the top-left corner of the window's content).
#[derive(Clone, Copy, Debug)]
struct PageStage {
    shown: bool,
    rect: Option<Rect>,
}

/// A drag in progress: where the pointer and the window were when it started, on the desktop.
#[derive(Clone, Copy)]
struct Drag {
    pointer: Point,
    window: Point,
}

/// A resize of the picture in progress: where the pointer was when it started (on the desktop), and the window then (on
/// its display), with its anchor.
#[derive(Clone)]
struct Resize {
    edge: Edge,
    pointer: Point,
    display: Display,
    window: Rect,
    anchor: Rect,
}

/// The card's state, and the signal its pointer watcher waits on while the card is hidden.
///
/// The lock is never held across a call that waits for the main thread: a window or monitor query (`outer_position`,
/// `available_monitors`, `cursor_position`, ...) from another thread waits for the main thread to answer it, and the
/// main thread takes this lock too (the card's commands, the room's level, clicks elsewhere), so the two waited on each
/// other for good (#170). The state is read and decided under the lock; the window is queried before and changed
/// after, with [`logic::Latest`] so that a change decided earlier never lands after a later one.
#[derive(Default)]
pub struct Card(Mutex<Inner>, Condvar);

/// Quitting: the card's window goes, with its webview.
pub fn close(app: &AppHandle) {
    if let Err(e) = NativeCardWindow::close(app, LABEL) {
        debug(&format!("call-controls not closed: {e}"));
    }
}

/// Creates the card's window, hidden; it shows during a call ([`update`]).
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let config_dir = crate::data::desktop_dir(app).ok();
    let placements = config_dir.as_deref().map(Placements::load).unwrap_or_default();
    let pointer_inside = if !NativeGeometry::capabilities().global_coordinates { None } else { Some(false) };
    app.manage(Card(
        Mutex::new(Inner { placements, config_dir, pointer_inside, ..Default::default() }),
        Condvar::new(),
    ));
    #[allow(unused_mut)] // the probe build adds its own
    let mut script = bridge::call_controls_script(APP_ORIGIN, NativeGeometry::capabilities().global_coordinates);
    #[cfg(feature = "probe")]
    if let Some(probe) = crate::probe::card_script() {
        script.push_str(probe);
    }
    NativeCardWindow::create(app, CardSpec { label: LABEL, page: PAGE, size: FIRST_SIZE, script })?;
    #[cfg(target_os = "macos")]
    {
        set_cursor_in_background();
        watch_clicks(app);
    }
    if NativeGeometry::capabilities().global_coordinates {
        watch_pointer(app.clone());
    }
    watch_view(app.clone());
    crate::debug_options::apply(app);
    debug("call-controls created");
    Ok(())
}

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

fn card(app: &AppHandle) -> Option<tauri::State<'_, Card>> {
    app.try_state::<Card>()
}

/// What the system shows of the room's window, for its stage (core `stage_on_screen`), as the log says it.
#[derive(Debug, Clone, Copy)]
struct Shown {
    shown: bool,
    minimised: bool,
    /// macOS: not fully covered nor on another Space; Windows: not on another virtual desktop; Wayland: focused.
    on_screen: bool,
    /// The part of the stage no other app's window covers, where the app can measure it (macOS).
    uncovered: Option<f64>,
}

impl Shown {
    fn stage_on_screen(&self) -> bool {
        logic::stage_on_screen(self.shown, self.minimised, self.on_screen, self.uncovered)
    }
}

/// Whether the person can see the room's stage (#165): the system shows it (core `stage_on_screen`) and the page does
/// (core `stage_in_view`). Which app has the focus is not part of it: the room in front with its stage, another app
/// focused, is still in view. With what the system said and what the page said, for the log; `None` once the app is
/// going.
///
/// The card's lock is held only to read what the page said; the window is asked on the main thread with it released
/// (#170).
fn stage_in_view(app: &AppHandle) -> Option<(bool, Shown, Option<bool>)> {
    let page = card(app).and_then(|card| card.0.lock().unwrap().page);
    let rect = page.and_then(|p| p.rect);
    let a = app.clone();
    let shown = on_main(app, move || system_shows(&a, rect))?;
    let page_shows = page.map(|p| p.shown);
    Some((logic::stage_in_view(shown.stage_on_screen(), page_shows), shown, page_shows))
}

/// Runs `f` on the main thread and waits for its answer (at once when called there). Never with a lock held that the
/// main thread may take.
fn on_main<T: Send + 'static>(app: &AppHandle, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .ok()?;
    rx.recv().ok()
}

/// On the main thread ([`stage_in_view`]): AppKit and the window handles want it. `stage` is where the page says its
/// stage is; without it, the room's whole content.
fn system_shows(app: &AppHandle, stage: Option<Rect>) -> Shown {
    let Some(w) = crate::room_window(app) else {
        return Shown { shown: false, minimised: false, on_screen: false, uncovered: None };
    };
    let shown = w.is_visible().unwrap_or(false);
    let minimised = w.is_minimized().unwrap_or(false);
    let on_screen = on_screen(&w);
    let uncovered = if shown && !minimised && on_screen { uncovered(&w, stage) } else { None };
    Shown { shown, minimised, on_screen, uncovered }
}

/// macOS: AppKit's occlusion state, clear while no part of the window is on screen (fully covered, or on another Space).
#[cfg(target_os = "macos")]
fn on_screen(w: &WebviewWindow) -> bool {
    use tauri_nspanel::objc2::{msg_send, runtime::AnyObject};
    // NSWindowOcclusionStateVisible: set while any part of the window is on screen.
    const VISIBLE: usize = 1 << 1;
    // SAFETY: the window's own NSWindow, read on the main thread ([`system_shows`]).
    w.ns_window().is_ok_and(|ns| unsafe {
        let state: usize = msg_send![ns as *mut AnyObject, occlusionState];
        state & VISIBLE != 0
    })
}

/// Windows: DWM cloaks a window on another virtual desktop. How much other windows cover it is not measured.
#[cfg(windows)]
fn on_screen(w: &WebviewWindow) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    let Ok(hwnd) = w.hwnd() else { return true };
    let mut cloaked: u32 = 0;
    // SAFETY: the room's own window handle, and a u32 as large as the attribute.
    let read = unsafe {
        DwmGetWindowAttribute(
            hwnd.0 as _,
            DWMWA_CLOAKED as _,
            (&mut cloaked as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    };
    read != 0 || cloaked == 0
}

/// Linux: X11 is not measured (a covered room counts as seen); Wayland shows the app nothing of other windows, so there
/// the room counts as seen only while it has the focus, as before #165.
#[cfg(target_os = "linux")]
fn on_screen(w: &WebviewWindow) -> bool {
    NativeGeometry::capabilities().global_coordinates || w.is_focused().unwrap_or(false)
}

/// macOS: the part of the stage (on the desktop) no other app's window above the room covers, from the window server's
/// list of the windows in front of it. Ordinary and floating windows count; the Dock, menus, the menu bar and the app's
/// own windows (the card) do not. Window bounds need no permission (only their titles would).
#[cfg(target_os = "macos")]
fn uncovered(w: &WebviewWindow, stage: Option<Rect>) -> Option<f64> {
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use tauri_nspanel::objc2::{msg_send, runtime::AnyObject};

    #[repr(C)]
    #[derive(Default)]
    struct CGRect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFArrayRef;
        fn CGRectMakeWithDictionaryRepresentation(dict: CFDictionaryRef, rect: *mut CGRect) -> bool;
    }
    // kCGWindowListOptionOnScreenAboveWindow | kCGWindowListExcludeDesktopElements.
    const ABOVE: u32 = (1 << 1) | (1 << 4);
    // kCGDockWindowLevel: the Dock and what floats over it (menus, the menu bar, the card) cover no window.
    const DOCK_LEVEL: i64 = 20;

    // The room's content on the desktop in points, top-left origin: as the window server gives windows' bounds.
    let scale = w.scale_factor().ok()?;
    let (at, size) = (w.inner_position().ok()?, w.inner_size().ok()?);
    let content = Rect {
        x: at.x as f64 / scale,
        y: at.y as f64 / scale,
        width: size.width as f64 / scale,
        height: size.height as f64 / scale,
    };
    let target = match stage {
        Some(s) => Rect { x: content.x + s.x, y: content.y + s.y, width: s.width, height: s.height },
        None => content,
    };
    // SAFETY: the room's own NSWindow, on the main thread.
    let number = w.ns_window().ok().map(|ns| -> isize { unsafe { msg_send![ns as *mut AnyObject, windowNumber] } })?;
    // SAFETY: CoreGraphics' own function; the array it creates is released by its wrapper.
    let raw = unsafe { CGWindowListCopyWindowInfo(ABOVE, u32::try_from(number).ok()?) };
    if raw.is_null() {
        return None;
    }
    let windows: CFArray<CFDictionary<CFString, CFType>> = unsafe { CFArray::wrap_under_create_rule(raw) };
    let own = i64::from(std::process::id());
    let number_of = |d: &CFDictionary<CFString, CFType>, key: &'static str| {
        d.find(CFString::from_static_string(key)).and_then(|v| v.downcast::<CFNumber>())
    };
    let above: Vec<Rect> = windows
        .iter()
        .filter_map(|d| {
            let layer = number_of(&d, "kCGWindowLayer")?.to_i64()?;
            let pid = number_of(&d, "kCGWindowOwnerPID")?.to_i64()?;
            let alpha = number_of(&d, "kCGWindowAlpha").and_then(|a| a.to_f64()).unwrap_or(1.0);
            if !(0..DOCK_LEVEL).contains(&layer) || pid == own || alpha <= 0.0 {
                return None;
            }
            let bounds = d.find(CFString::from_static_string("kCGWindowBounds"))?;
            if !bounds.instance_of::<CFDictionary>() {
                return None;
            }
            let mut r = CGRect::default();
            // SAFETY: a CFDictionary (checked above), as CoreGraphics wrote it.
            unsafe { CGRectMakeWithDictionaryRepresentation(bounds.as_CFTypeRef() as CFDictionaryRef, &mut r) }
                .then_some(Rect { x: r.x, y: r.y, width: r.width, height: r.height })
        })
        .collect();
    Some(logic::uncovered(target, &above))
}

#[cfg(not(target_os = "macos"))]
fn uncovered(_: &WebviewWindow, _: Option<Rect>) -> Option<f64> {
    None
}

/// Minimising, covering, Space changes and windows of other apps moving raise no window event Tauri forwards, so the app
/// looks at the stage twice a second and updates the card when it comes into view or leaves it. One thread for the app's
/// lifetime.
fn watch_view(app: AppHandle) {
    std::thread::spawn(move || {
        let mut was = None;
        loop {
            std::thread::sleep(VIEW_EVERY);
            let Some((now, shown, page)) = stage_in_view(&app) else { return };
            if was.replace(now).is_some_and(|before| before != now) {
                debug(&format!("call-controls stage in view={now}: {shown:?} page={page:?}"));
                update(&app);
            }
        }
    });
}

/// Shows or hides the card for the call as it is now, and tells it what changed. Called on every change of the call,
/// of the room window's view, of the settings, of the card's × and of the tray's "hide".
pub fn update(app: &AppHandle) {
    let Some(card) = card(app) else { return };
    let call = app.state::<AppState>().call.lock().unwrap().clone();
    let Some((in_view, _, _)) = stage_in_view(app) else { return };
    let always_shown =
        app.state::<AppState>().settings.lock().unwrap().as_ref().is_some_and(|s| s.call_controls_always_shown);
    // Switched off in Settings: no card at all, not even the moment a change shows it.
    let enabled = app.state::<AppState>().settings.lock().unwrap().as_ref().is_none_or(|s| s.call_controls_enabled);
    let (show, hide, unhidden, reason, decision) = {
        let mut inner = card.0.lock().unwrap();
        let hidden = logic::still_hidden(inner.hidden, call.joined, inner.stage_in_view, in_view);
        let unhidden = inner.hidden && !hidden;
        inner.hidden = hidden;
        inner.stage_in_view = Some(in_view);
        let mut previewing = inner.preview_until.is_some_and(|until| Instant::now() < until);
        // Never while the person is at it: the pointer over it, a drag or a resize keep it, and it goes
        // PREVIEW_AFTER_USE after they are done.
        if previewing && (inner.drag.is_some() || inner.resize.is_some() || inner.pointer_inside == Some(true)) {
            inner.preview_until = Some(Instant::now() + PREVIEW_AFTER_USE);
        }
        if !previewing {
            inner.preview_until = None;
            previewing = false;
        }
        let by_rule = logic::wanted(enabled, call.joined, inner.hidden, in_view, always_shown);
        let wanted = by_rule || (enabled && call.joined && previewing);
        let change = (wanted && !inner.shown, !wanted && inner.shown);
        inner.shown = wanted;
        let decision = if change.0 || change.1 { inner.visibility.advance() } else { 0 };
        if change.1 {
            // Nothing of this showing carries over to the next: no pointer over it, no level, no move.
            inner.drag = None;
            inner.resize = None;
            inner.level = 0;
            inner.hand = false;
            inner.told_place = None;
            inner.told_size = None;
            if inner.pointer_inside.is_some() {
                inner.pointer_inside = Some(false);
            }
        }
        let reason = if wanted && !by_rule {
            "preview"
        } else {
            logic::why(enabled, call.joined, inner.hidden, in_view, always_shown)
        };
        (change.0, change.1, unhidden, reason, decision)
    };
    if show {
        place(app);
        settle_size(app);
        apply_visible(app, true, decision);
        debug(&format!("call-controls shown: {reason}"));
        card.1.notify_all();
    } else if hide {
        apply_visible(app, false, decision);
        debug(&format!("call-controls hidden: {reason}"));
    }
    if unhidden {
        crate::tray::update(app, &call);
    }
    push(app, full_state(app, &call));
}

/// Puts the window on the screen or takes it off, on the main thread, but only if no later show or hide was decided
/// meanwhile: [`update`] runs on several threads (the call, the room's view, a preview, the page's commands), and one
/// that decided "show" can reach the window after another that decided "hide" (#161). Every decision queues its own
/// apply after taking its number, so the latest one always lands and window and state cannot disagree; and no lock is
/// held while the main thread is asked (#170).
fn apply_visible(app: &AppHandle, visible: bool, decision: u64) {
    let a = app.clone();
    let _ = app.run_on_main_thread(move || {
        let latest = card(&a).is_some_and(|card| card.0.lock().unwrap().visibility.is(decision));
        if latest {
            NativeCardWindow::set_visible(&a, LABEL, visible);
        } else {
            debug(&format!("call-controls {} skipped: changed meanwhile", if visible { "show" } else { "hide" }));
        }
    });
}

/// Moves the window there, on the main thread, unless a later move was decided meanwhile (as [`apply_visible`]).
fn move_when_latest(app: &AppHandle, at: Point, decision: u64) {
    let a = app.clone();
    let _ = app.run_on_main_thread(move || {
        let latest = card(&a).is_some_and(|card| card.0.lock().unwrap().moves.is(decision));
        if let (true, Some(w)) = (latest, window(&a)) {
            NativeGeometry::place(&w, at);
        }
    });
}

/// The window is as big as the page last asked (`layout`) when it shows. The page asks only when its own size changes,
/// and a page that is not on screen may not notice a change until it is (a size picked in Settings while the card was
/// hidden), so the window could be left at a size the page no longer has: its content then sits misplaced in it or is cut
/// at its edge. What the window really is is read back and corrected, and logged, so a mismatch is seen.
fn settle_size(app: &AppHandle) {
    let (Some(card), Some(w)) = (card(app), window(app)) else { return };
    let Some(wanted) = card.0.lock().unwrap().size else { return };
    let Some(actual) = NativeGeometry::frame(&w) else { return };
    let unit = NativeGeometry::unit(&w);
    let (aw, ah) = (actual.width / unit, actual.height / unit);
    if (aw - wanted.width).abs() > 1.0 || (ah - wanted.height).abs() > 1.0 {
        debug(&format!("call-controls window was {aw:.0}x{ah:.0}, set to {:.0}x{:.0}", wanted.width, wanted.height));
        let _ = w.set_size(LogicalSize::new(wanted.width, wanted.height));
        place(app);
    }
}

/// The tray's "Hide / Show call controls".
pub fn toggle_hidden(app: &AppHandle) {
    if let Some(card) = card(app) {
        let mut inner = card.0.lock().unwrap();
        inner.hidden = !inner.hidden;
    }
    update(app);
}

/// The card's ×: hidden until the stage comes into view or leaves it again, or the next call.
fn hide(app: &AppHandle) {
    if let Some(card) = card(app) {
        card.0.lock().unwrap().hidden = true;
    }
    update(app);
    let call = app.state::<AppState>().call.lock().unwrap().clone();
    crate::tray::update(app, &call);
}

/// Whether the card is hidden for now (the tray's label says the opposite).
pub fn hidden_for_call(app: &AppHandle) -> bool {
    card(app).is_some_and(|card| card.0.lock().unwrap().hidden)
}

/// Settings → Floating card changed something: during a call, the card shows a moment, in front of the room too.
fn preview(app: &AppHandle) {
    let Some(card) = card(app) else { return };
    let watch = {
        let mut inner = card.0.lock().unwrap();
        // Each change starts the time again, from now.
        inner.preview_until = Some(Instant::now() + PREVIEW_FOR);
        !std::mem::replace(&mut inner.preview_watched, true)
    };
    place(app);
    update(app);
    if !watch {
        return;
    }
    // Looked at again and again while it shows: an interaction extends it (in `update`), and it goes when its time is up.
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(PREVIEW_EVERY);
        update(&app);
        let Some(card) = self::card(&app) else { return };
        let mut inner = card.0.lock().unwrap();
        if inner.preview_until.is_none() {
            inner.preview_watched = false;
            return;
        }
    });
}

/// Leaving Settings → Floating card: a preview showing goes at once.
fn end_preview(app: &AppHandle) {
    let Some(card) = card(app) else { return };
    card.0.lock().unwrap().preview_until = None;
    update(app);
}

/// The room's page says whether it shows its stage, and where (#165): Settings and the app's other screens without it
/// bring the card up as the room going out of view does. The card changes only when what the page shows changes; a
/// stage that only moved or resized is measured at the next look ([`watch_view`]).
fn stage_seen_by_page(app: &AppHandle, shown: bool, rect: Option<CardBox>) {
    let Some(card) = card(app) else { return };
    let rect = rect
        .map(|b| Rect { x: b.x, y: b.y, width: b.width, height: b.height })
        .filter(|r| [r.x, r.y, r.width, r.height].iter().all(|v| v.is_finite() && v.abs() < MAX_SIDE))
        .filter(|r| r.width > 0.0 && r.height > 0.0);
    let before = card.0.lock().unwrap().page.replace(PageStage { shown, rect });
    if before.is_none_or(|b| b.shown != shown) {
        debug(&format!("call-controls page shows the stage={shown}"));
        update(app);
    }
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

/// The voice's output level from the room window (0–1, and whose reply it is), for the avatar's mouth.
pub fn playback(app: &AppHandle, level: f64, thread_id: Option<String>) {
    if card(app).is_some_and(|card| card.0.lock().unwrap().shown) {
        push(app, serde_json::json!({ "playback": { "level": level, "threadId": thread_id } }));
    }
}

fn full_state(app: &AppHandle, call: &CallSnapshot) -> serde_json::Value {
    let settings =
        app.state::<AppState>().settings.lock().unwrap().clone().unwrap_or_else(settings::Settings::first_run);
    let (level, pointer, clicks, place, size) = card(app)
        .map(|card| {
            let inner = card.0.lock().unwrap();
            (inner.level, inner.pointer_inside, inner.outside_clicks, inner.told_place, inner.told_size.flatten())
        })
        .unwrap_or((0, None, 0, None, None));
    let mut state = serde_json::json!({
        "call": call,
        "level": level,
        "pointerInside": pointer,
        "outsideClicks": clicks,
        "alwaysExpanded": settings.call_controls_always_shown,
        "muteShortcut": logic::shortcut_label(&settings.mute_shortcut, cfg!(target_os = "macos")),
        "size": size.map(size_json),
    });
    if let Some(place) = place {
        state["place"] = place.place();
    }
    state
}

fn size_json(size: Size) -> serde_json::Value {
    serde_json::json!({ "width": size.width.round(), "height": size.height.round() })
}

fn push(app: &AppHandle, patch: serde_json::Value) {
    if let Some(w) = window(app) {
        let _ = w.eval(bridge::call_controls_push(&patch));
    }
}

/// The preferred corner: where a card first appears, and where a click on a corner in Settings puts it.
fn default_corner(app: &AppHandle) -> Corner {
    app.state::<AppState>().settings.lock().unwrap().as_ref().map(|s| s.call_controls_corner).unwrap_or_default()
}

/// Puts the card where it rests on its display (where it was left there, or the default corner the first time): its
/// body there, the window around it, moved just enough for all of it to fit. Then tells the card its place and its
/// resized size there, when they changed. On Wayland the compositor places it.
fn place(app: &AppHandle) {
    if !NativeGeometry::capabilities().global_coordinates {
        return;
    }
    let corner = default_corner(app);
    let Some(card) = card(app) else { return };
    if window(app).is_none() {
        return;
    }
    let key = card.0.lock().unwrap().rest.as_ref().map(|(d, _)| d.key.clone());
    // The monitors are the main thread's to answer: asked with the lock released (#170).
    let Some(display) = NativeGeometry::display(app, crate::room_window(app).as_ref(), key.as_deref(), None) else {
        return;
    };
    let (to, decision, patch) = {
        let mut inner = card.0.lock().unwrap();
        let size = inner.size.unwrap_or(FIRST_SIZE);
        let anchor = inner.anchor.unwrap_or_else(|| logic::anchor(size, None, MARGIN));
        let spot = match &inner.rest {
            Some((d, spot)) if d.key == display.key => *spot,
            _ => inner.placements.displays.get(&display.key).copied().unwrap_or(Spot::corner(corner)),
        };
        let rest = spot.at(Size { width: anchor.width, height: anchor.height }, display.work);
        let at = logic::window_at(rest, anchor, size, display.work);
        let to = display.to_desktop(at);
        debug(&format!(
            "call-controls at {:.0},{:.0} on {} size {:.0}x{:.0} rest {:.0},{:.0}",
            at.x, at.y, display.key, size.width, size.height, rest.x, rest.y
        ));
        let resized = inner.placements.sizes.get(&display.key).copied();
        inner.rest = Some((display, spot));
        let mut patch = serde_json::Map::new();
        if inner.told_place != Some(spot.corner) {
            inner.told_place = Some(spot.corner);
            patch.insert("place".into(), spot.corner.place());
        }
        if inner.told_size != Some(resized) {
            inner.told_size = Some(resized);
            patch.insert("size".into(), resized.map(size_json).unwrap_or(serde_json::Value::Null));
        }
        (to, inner.moves.advance(), patch)
    };
    move_when_latest(app, to, decision);
    if !patch.is_empty() {
        push(app, serde_json::Value::Object(patch));
    }
}

/// Keeps where the card rests and its sizes, for the next launch.
fn save(inner: &Inner) {
    if let Some(dir) = &inner.config_dir {
        if let Err(e) = inner.placements.save(dir) {
            debug(&format!("call-controls placement not saved: {e}"));
        }
    }
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

/// `host.callControls`' one command, from the room page (not the card), each kept with the app's settings or its
/// placements and applied at once: `settings` (answers `{alwaysShown, enabled, corner, moved}`), `set-always-shown` with
/// `enabled` (`{alwaysShown}`), `set-enabled` with `enabled` (`{enabled}`: whether the card shows at all), `set-corner` with `corner` (`{corner}`), `reset-place` (forgets where the card was left
/// and how the picture was resized, on every display), `clear-size` (forgets only the picture's resized sizes), `preview` (the card a moment, during a call),
/// and `stage` with `visible` and `rect` (whether the page shows its stage, and where: #165).
#[tauri::command]
pub async fn call_controls_settings(
    app: AppHandle,
    webview: Webview,
    action: String,
    enabled: Option<bool>,
    corner: Option<Corner>,
    visible: Option<bool>,
    rect: Option<CardBox>,
) -> Result<serde_json::Value, serde_json::Value> {
    crate::room_page(&app, &webview).map_err(|why| crate::refusal("bad_request", why))?;
    if action == "stage" {
        let visible = visible.ok_or_else(|| crate::refusal("bad_request", "stage takes visible"))?;
        stage_seen_by_page(&app, visible, rect);
        return Ok(serde_json::Value::Null);
    }
    let stored =
        || app.state::<AppState>().settings.lock().unwrap().clone().unwrap_or_else(settings::Settings::first_run);
    match (action.as_str(), enabled, corner) {
        ("settings", _, _) => {
            let s = stored();
            let moved = card(&app).is_some_and(|card| card.0.lock().unwrap().placements.moved());
            Ok(
                serde_json::json!({ "alwaysShown": s.call_controls_always_shown, "enabled": s.call_controls_enabled, "corner": s.call_controls_corner, "moved": moved }),
            )
        }
        ("set-always-shown", Some(enabled), _) => {
            crate::change_settings(&app, |s| s.call_controls_always_shown = enabled)
                .map_err(|e| crate::refusal("internal", e))?;
            update(&app);
            Ok(serde_json::json!({ "alwaysShown": enabled }))
        }
        ("set-enabled", Some(enabled), _) => {
            crate::change_settings(&app, |s| s.call_controls_enabled = enabled)
                .map_err(|e| crate::refusal("internal", e))?;
            update(&app);
            Ok(serde_json::json!({ "enabled": enabled }))
        }
        ("set-corner", _, Some(corner)) => {
            crate::change_settings(&app, |s| s.call_controls_corner = corner)
                .map_err(|e| crate::refusal("internal", e))?;
            // Clicking a corner, the chosen one too, moves the card there now and makes it the preferred one: the places
            // dragged to are forgotten (the sizes are kept). A drag after that leaves the card where it is dropped, the
            // corner still chosen, until a corner is clicked again or "Reset size and position".
            if let Some(card) = card(&app) {
                let mut inner = card.0.lock().unwrap();
                inner.placements.displays.clear();
                if let Some((display, _)) = inner.rest.take() {
                    inner.rest = Some((display, Spot::corner(corner)));
                }
                save(&inner);
            }
            place(&app);
            Ok(serde_json::json!({ "corner": corner }))
        }
        ("reset-place", _, _) => {
            if let Some(card) = card(&app) {
                let mut inner = card.0.lock().unwrap();
                inner.placements = Placements::default();
                inner.rest = None;
                save(&inner);
            }
            place(&app);
            Ok(serde_json::Value::Null)
        }
        ("clear-size", _, _) => {
            if let Some(card) = card(&app) {
                let mut inner = card.0.lock().unwrap();
                inner.placements.sizes.clear();
                save(&inner);
            }
            place(&app);
            Ok(serde_json::Value::Null)
        }
        ("end-preview", _, _) => {
            end_preview(&app);
            Ok(serde_json::Value::Null)
        }
        ("preview", _, _) => {
            preview(&app);
            Ok(serde_json::Value::Null)
        }
        _ => Err(crate::refusal("bad_request", format!("not a call controls action: {action}"))),
    }
}

/// The card's page is ready: it gets the whole state.
#[tauri::command]
pub fn call_controls_ready(app: AppHandle, webview: Webview) -> Result<(), String> {
    from_card_window(&webview)?;
    let call = app.state::<AppState>().call.lock().unwrap().clone();
    push(&app, full_state(&app, &call));
    Ok(())
}

/// Whether the pointer is over something clickable in the card (its CSS cursor is the hand). The app sets the hand
/// itself while the pointer is over the card ([`keep_hand`]).
#[tauri::command]
pub fn call_controls_cursor(app: AppHandle, webview: Webview, hand: bool) -> Result<(), String> {
    from_card_window(&webview)?;
    if let Some(card) = card(&app) {
        card.0.lock().unwrap().hand = hand;
    }
    Ok(())
}

/// One of the card's commands: "open-app", "hide" and "dismiss" are the app's, "open-settings" brings the room in front first, the
/// rest are carried to the room page.
#[tauri::command]
pub fn call_controls_run(app: AppHandle, webview: Webview, command: serde_json::Value) -> Result<(), String> {
    from_card_window(&webview)?;
    match command.get("command").and_then(|c| c.as_str()) {
        Some("open-app") => {
            crate::show_room(&app);
            return Ok(());
        }
        Some("hide") => {
            debug("call-controls run hide");
            hide(&app);
            return Ok(());
        }
        // The exit notice's Close, once a call ended by itself: the card goes at once (it never outlasts the call).
        Some("dismiss") => {
            debug("call-controls run dismiss");
            update(&app);
            return Ok(());
        }
        _ => {}
    }
    let command: Command = serde_json::from_value(command.clone()).map_err(|e| {
        debug(&format!("call-controls refused {command}: {e}"));
        format!("not a card command: {e}")
    })?;
    debug(&format!("call-controls run {} {}", command.name(), NativeCardWindow::app_active()));
    if command == Command::OpenSettings {
        crate::show_room(&app);
    }
    crate::send(&app, command);
    Ok(())
}

/// Where the card's body is inside its window (`layout`'s `card`), in CSS pixels.
#[derive(Deserialize)]
pub struct CardBox {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// The picture-in-picture, which the person may resize (`call_controls_resize`) down to `minWidth`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resizable {
    min_width: f64,
    min_height: f64,
}

/// The card's size with its margin, where its body is in it, and whether it may be resized: the window takes the size,
/// and is placed with its body where it rests.
#[tauri::command]
pub fn call_controls_layout(
    app: AppHandle,
    webview: Webview,
    width: f64,
    height: f64,
    card: Option<CardBox>,
    resizable: Option<Resizable>,
) -> Result<(), String> {
    from_card_window(&webview)?;
    let side = |v: f64| v.is_finite() && v > 0.0 && v < MAX_SIDE;
    if !(side(width) && side(height)) {
        return Err("not a card size".into());
    }
    let size = Size { width, height };
    let body = card.filter(|b| [b.x, b.y, b.width, b.height].iter().all(|v| v.is_finite())).map(|b| Rect {
        x: b.x,
        y: b.y,
        width: b.width,
        height: b.height,
    });
    let resizing = if let Some(card) = self::card(&app) {
        let mut inner = card.0.lock().unwrap();
        inner.size = Some(size);
        inner.anchor = Some(logic::anchor(size, body, MARGIN));
        inner.min_width = resizable.as_ref().map(|r| r.min_width).filter(|w| side(*w));
        inner.min_height = resizable.map(|r| r.min_height).filter(|h| side(*h));
        inner.resize.is_some()
    } else {
        false
    };
    if let Some(w) = window(&app) {
        let _ = w.set_size(LogicalSize::new(width, height));
    }
    // While the person resizes the picture, the resize places the window.
    if !resizing {
        place(&app);
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DragPhase {
    Start,
    Move,
    End,
}

/// Moving the card: the window follows the pointer; dropped, its body snaps (free with magnet) and is remembered for
/// that display. The app follows the pointer itself, on the desktop, so displays of different scales agree. On Wayland
/// the compositor moves the window, and where it is left is not the app's to know.
#[tauri::command]
pub fn call_controls_drag(app: AppHandle, webview: Webview, phase: DragPhase) -> Result<(), String> {
    from_card_window(&webview)?;
    let (Some(card), Some(w)) = (card(&app), window(&app)) else { return Ok(()) };
    if !NativeGeometry::capabilities().global_coordinates {
        if let DragPhase::Start = phase {
            let _ = NativeGeometry::start_dragging(&w);
        }
        return Ok(());
    }
    // The pointer, the window and the monitors are asked with the card's lock released (#170).
    match phase {
        DragPhase::Start => {
            let drag = NativeGeometry::pointer(&app)
                .zip(NativeGeometry::frame(&w))
                .map(|(pointer, at)| Drag { pointer, window: Point { x: at.x, y: at.y } });
            card.0.lock().unwrap().drag = drag;
        }
        DragPhase::Move => {
            let drag = card.0.lock().unwrap().drag;
            if let (Some(drag), Some(p)) = (drag, NativeGeometry::pointer(&app)) {
                let to = Point { x: drag.window.x + p.x - drag.pointer.x, y: drag.window.y + p.y - drag.pointer.y };
                let decision = card.0.lock().unwrap().moves.advance();
                move_when_latest(&app, to, decision);
            }
        }
        DragPhase::End => {
            let drag = card.0.lock().unwrap().drag.take();
            let (Some(drag), Some(p)) = (drag, NativeGeometry::pointer(&app)) else { return Ok(()) };
            let dropped = Point { x: drag.window.x + p.x - drag.pointer.x, y: drag.window.y + p.y - drag.pointer.y };
            // The display the pointer let go on; the card's corner may be just off it.
            let Some(display) = NativeGeometry::display(&app, crate::room_window(&app).as_ref(), None, Some(p)) else {
                return Ok(());
            };
            let mut inner = card.0.lock().unwrap();
            let size = inner.size.unwrap_or(FIRST_SIZE);
            let anchor = inner.anchor.unwrap_or_else(|| logic::anchor(size, None, MARGIN));
            let body = Size { width: anchor.width, height: anchor.height };
            let window = display.to_local(dropped);
            let rest = logic::drop_at(Point { x: window.x + anchor.x, y: window.y + anchor.y }, body, display.work);
            let spot = Spot::of(rest, body, display.work);
            inner.placements.displays.insert(display.key.clone(), spot);
            save(&inner);
            debug(&format!(
                "call-controls dropped at {:.0},{:.0} on {} {}",
                rest.x,
                rest.y,
                display.key,
                NativeCardWindow::app_active()
            ));
            inner.rest = Some((display, spot));
            drop(inner);
            place(&app);
        }
    }
    Ok(())
}

/// The corner that stays still while the picture is resized by `edge`.
fn opposite(edge: Edge) -> Corner {
    match edge {
        Edge::Nw => Corner::BottomRight,
        Edge::Ne => Corner::BottomLeft,
        Edge::Sw => Corner::TopRight,
        Edge::Se => Corner::TopLeft,
    }
}

/// Resizing the picture-in-picture by its grip: the window follows the pointer with its opposite corner still, free in both directions
/// (the page fits its 16:9 stage inside) and at least the page's least width and height (core `resized`); the card hears its new size (`size`), and
/// the size and where it rests are remembered for that display. Only while the page says the picture may be resized;
/// not on Wayland (the card's bridge offers no `resize` there).
#[tauri::command]
pub fn call_controls_resize(app: AppHandle, webview: Webview, phase: DragPhase, edge: Edge) -> Result<(), String> {
    from_card_window(&webview)?;
    if !NativeGeometry::capabilities().global_coordinates {
        return Ok(());
    }
    let (Some(card), Some(w)) = (card(&app), window(&app)) else { return Ok(()) };
    let (min_width, min_height, size, anchor, display, resize) = {
        let inner = card.0.lock().unwrap();
        let (Some(min_width), Some(min_height)) = (inner.min_width, inner.min_height) else {
            return Err("the card cannot be resized now".into());
        };
        let size = inner.size.unwrap_or(FIRST_SIZE);
        let anchor = inner.anchor.unwrap_or_else(|| logic::anchor(size, None, MARGIN));
        (min_width, min_height, size, anchor, inner.rest.as_ref().map(|(d, _)| d.clone()), inner.resize.clone())
    };
    // The pointer and the window are asked with the card's lock released (#170).
    match phase {
        DragPhase::Start => {
            let resize = NativeGeometry::pointer(&app).zip(NativeGeometry::frame(&w)).zip(display).map(
                |((pointer, at), display)| {
                    let corner = display.to_local(Point { x: at.x, y: at.y });
                    let window = Rect { x: corner.x, y: corner.y, width: size.width, height: size.height };
                    Resize { edge, pointer, display, window, anchor }
                },
            );
            card.0.lock().unwrap().resize = resize;
        }
        DragPhase::Move | DragPhase::End => {
            let (Some(resize), Some(p)) = (resize, NativeGeometry::pointer(&app)) else { return Ok(()) };
            let dx = (p.x - resize.pointer.x) / resize.display.unit;
            let dy = (p.y - resize.pointer.y) / resize.display.unit;
            let to = logic::resized(resize.window, resize.edge, dx, dy, min_width, min_height, resize.display.work);
            let size = Size { width: to.width, height: to.height };
            let a = resize.anchor;
            let anchor = Rect {
                x: a.x,
                y: a.y,
                width: a.width + to.width - resize.window.width,
                height: a.height + to.height - resize.window.height,
            };
            let rest = Point { x: to.x + anchor.x, y: to.y + anchor.y };
            let body = Size { width: anchor.width, height: anchor.height };
            let spot = Spot::against(opposite(resize.edge), rest, body, resize.display.work);
            let at = resize.display.to_desktop(Point { x: to.x, y: to.y });
            let mut inner = card.0.lock().unwrap();
            let decision = inner.moves.advance();
            inner.size = Some(size);
            inner.anchor = Some(anchor);
            inner.placements.sizes.insert(resize.display.key.clone(), size);
            inner.told_size = Some(Some(size));
            if let DragPhase::End = phase {
                inner.resize = None;
                inner.placements.displays.insert(resize.display.key.clone(), spot);
                save(&inner);
                debug(&format!("call-controls resized to {:.0}x{:.0} on {}", to.width, to.height, resize.display.key));
            }
            inner.rest = Some((resize.display, spot));
            drop(inner);
            let _ = w.set_size(LogicalSize::new(to.width, to.height));
            move_when_latest(&app, at, decision);
            push(&app, serde_json::json!({ "size": size_json(size) }));
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

/// macOS: lets the app set the cursor while it is not the active app (#139). The window server ignores a cursor set
/// by an app in the background, so the hand over the card's buttons never showed; `SetsCursorInBackground` is the
/// connection property it honours instead. Private, looked up at run time: where it is gone the card only keeps the
/// arrow. WebKit sets a cursor only while the pointer is over its own window, and the app puts the arrow back when the
/// pointer leaves the card ([`arrow_after_leaving`]).
#[cfg(target_os = "macos")]
fn set_cursor_in_background() {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::string::CFString;
    type MainConnection = unsafe extern "C" fn() -> i32;
    type SetProperty = unsafe extern "C" fn(i32, i32, *const std::ffi::c_void, *const std::ffi::c_void) -> i32;
    let symbol = |name: &std::ffi::CStr| unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    let main = symbol(c"CGSMainConnectionID");
    let set = symbol(c"CGSSetConnectionProperty");
    let result = if main.is_null() || set.is_null() {
        "missing".to_string()
    } else {
        // SAFETY: CoreGraphics' own functions, with the signatures it has had since Mac OS X 10.4.
        unsafe {
            let main = std::mem::transmute::<*mut std::ffi::c_void, MainConnection>(main);
            let set = std::mem::transmute::<*mut std::ffi::c_void, SetProperty>(set);
            let connection = main();
            let key = CFString::from_static_string("SetsCursorInBackground");
            let error = set(
                connection,
                connection,
                key.as_concrete_TypeRef().cast(),
                CFBoolean::true_value().as_concrete_TypeRef().cast(),
            );
            if error == 0 {
                "true".to_string()
            } else {
                format!("error {error}")
            }
        }
    };
    debug(&format!("call-controls sets the cursor in background: {result}"));
}

/// macOS: the pointer left the card: the cursor it showed there (a hand over a button) goes back to the arrow, since the
/// app under the pointer may not set its own until the pointer enters one of its views again.
#[cfg(target_os = "macos")]
fn arrow_after_leaving(app: &AppHandle) {
    let _ = app.run_on_main_thread(|| {
        use tauri_nspanel::objc2::{class, msg_send, runtime::AnyObject};
        // SAFETY: NSCursor's class methods, on the main thread.
        unsafe {
            let current: *mut AnyObject = msg_send![class!(NSCursor), currentCursor];
            let arrow: *mut AnyObject = msg_send![class!(NSCursor), arrowCursor];
            if current != arrow {
                let _: () = msg_send![arrow, set];
            }
        }
    });
}

/// macOS: the hand over the card's buttons while another app is in front (#166). WebKit sets the card's cursor only when
/// the pointer moves over it, and an app in front that sets its own cursor on every mouse move (a text editor's I-beam)
/// takes it back at once, so the hand is set again at each look at the pointer ([`logic::must_set_hand`]).
#[cfg(target_os = "macos")]
fn keep_hand(app: &AppHandle, inside: Option<bool>, page_wants_hand: bool) {
    if inside != Some(true) || !page_wants_hand {
        return;
    }
    let _ = app.run_on_main_thread(move || {
        use tauri_nspanel::objc2::{class, msg_send, runtime::AnyObject};
        // SAFETY: NSCursor's class methods, on the main thread.
        unsafe {
            let current: *mut AnyObject = msg_send![class!(NSCursor), currentCursor];
            let hand: *mut AnyObject = msg_send![class!(NSCursor), pointingHandCursor];
            if logic::must_set_hand(inside, page_wants_hand, current == hand) {
                let _: () = msg_send![hand, set];
            }
        }
    });
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
            let inside = NativeGeometry::pointer(&app).zip(NativeGeometry::frame(&w)).map(|(p, at)| {
                let margin = MARGIN * NativeGeometry::unit(&w);
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
            #[cfg(target_os = "macos")]
            {
                let hand = card(&app).is_some_and(|card| card.0.lock().unwrap().hand);
                keep_hand(&app, inside, hand);
            }
            if changed {
                debug(&format!("call-controls pointer inside={inside:?} {}", NativeCardWindow::app_active()));
                push(&app, serde_json::json!({ "pointerInside": inside }));
                #[cfg(target_os = "macos")]
                if inside == Some(false) {
                    arrow_after_leaving(&app);
                }
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
