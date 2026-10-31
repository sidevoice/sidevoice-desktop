//! The call controls card's pure logic (sidevoice/sidevoice-desktop#4): when it shows, where it sits, how it moves
//! and grows, where it is remembered, and how the mute shortcut reads on it. The window itself is `src/call_controls.rs`.
//!
//! Geometry is in logical pixels of one display, relative to the top-left corner of its work area (y down). The card is
//! placed by its body with its shadow margin ([`anchor`]); its *window* grows around that body (captions, a list, the
//! avatar's dock), which stays where it rests ([`window_at`]). Each display is its own space, so displays
//! of different scales never mix units; [`Display`] converts to and from the desktop's physical pixels.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Dropped within this distance of an edge of the work area, the card snaps to it ("free with magnet").
pub const MAGNET: f64 = 56.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

/// A display's work area: the screen minus the menu bar, the Dock or the taskbar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn right(&self) -> f64 {
        self.x + self.width
    }
    fn bottom(&self) -> f64 {
        self.y + self.height
    }
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }
}

/// A display as the card sees it: a stable key, and its work area both on the desktop and in its own logical pixels.
///
/// The desktop is the one space where every display sits side by side without gaps or overlaps, whatever their scales:
/// physical pixels on Windows and X11, points (logical pixels) on macOS. `unit` is how many desktop units one of this
/// display's logical pixels takes: its scale on Windows and X11, 1 on macOS.
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    pub key: String,
    pub unit: f64,
    /// The work area's top-left corner, on the desktop.
    pub origin: Point,
    /// The work area in this display's logical pixels, from its own top-left corner (x and y are 0).
    pub work: Rect,
}

impl Display {
    /// `name` (two displays of one model share it) with where it is on the desktop, so each keeps its own place.
    pub fn new(name: &str, unit: f64, origin: Point, size: Size) -> Self {
        Display {
            key: format!("{name}@{:.0},{:.0}", origin.x, origin.y),
            unit: if unit > 0.0 { unit } else { 1.0 },
            origin,
            work: Rect { x: 0.0, y: 0.0, width: size.width, height: size.height },
        }
    }
    /// A point of this display (logical, from its work area's corner) on the desktop.
    pub fn to_desktop(&self, p: Point) -> Point {
        Point { x: self.origin.x + p.x * self.unit, y: self.origin.y + p.y * self.unit }
    }
    /// A point of the desktop in this display's logical pixels, from its work area's corner.
    pub fn to_local(&self, p: Point) -> Point {
        Point { x: (p.x - self.origin.x) / self.unit, y: (p.y - self.origin.y) / self.unit }
    }
}

/// A corner of a display's work area: where a card that was never moved there first appears (Settings → Floating card →
/// Default position), and the corner a placed card is kept against.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Corner {
    #[serde(rename = "tl")]
    TopLeft,
    #[default]
    #[serde(rename = "tr")]
    TopRight,
    #[serde(rename = "bl")]
    BottomLeft,
    #[serde(rename = "br")]
    BottomRight,
}

impl Corner {
    fn of(left: bool, top: bool) -> Corner {
        match (top, left) {
            (true, true) => Corner::TopLeft,
            (true, false) => Corner::TopRight,
            (false, true) => Corner::BottomLeft,
            (false, false) => Corner::BottomRight,
        }
    }
    fn left(self) -> bool {
        matches!(self, Corner::TopLeft | Corner::BottomLeft)
    }
    fn top(self) -> bool {
        matches!(self, Corner::TopLeft | Corner::TopRight)
    }
    /// The card's `place` (call-controls/host.ts): the half of the work area it sits in, which its captions and lists
    /// open away from.
    pub fn place(self) -> serde_json::Value {
        serde_json::json!({
            "vertical": if self.top() { "upper" } else { "lower" },
            "horizontal": if self.left() { "left" } else { "right" },
        })
    }
}

/// Where the card rests on a display: the corner of the work area nearest to it, and how far its own corner is from
/// that one. A card against an edge stays against it whatever size it takes (another format, another size), and a card
/// that grows grows away from its corner, toward the middle of the screen.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Spot {
    pub corner: Corner,
    pub dx: f64,
    pub dy: f64,
}

impl Spot {
    /// A first place: in that corner.
    pub fn corner(corner: Corner) -> Spot {
        Spot { corner, dx: 0.0, dy: 0.0 }
    }
    /// The spot of a card of `size` at `at`, against the corner of the half its middle is in.
    pub fn of(at: Point, size: Size, work: Rect) -> Spot {
        let left = at.x + size.width / 2.0 < work.x + work.width / 2.0;
        let top = at.y + size.height / 2.0 < work.y + work.height / 2.0;
        Spot::against(Corner::of(left, top), at, size, work)
    }
    /// The spot of a card of `size` at `at`, against `corner`.
    pub fn against(corner: Corner, at: Point, size: Size, work: Rect) -> Spot {
        let dx = if corner.left() { at.x - work.x } else { work.right() - (at.x + size.width) };
        let dy = if corner.top() { at.y - work.y } else { work.bottom() - (at.y + size.height) };
        Spot { corner, dx, dy }
    }
    /// Where a card of `size` at this spot has its top-left corner, kept inside the work area.
    pub fn at(&self, size: Size, work: Rect) -> Point {
        let x = if self.corner.left() { work.x + self.dx } else { work.right() - self.dx - size.width };
        let y = if self.corner.top() { work.y + self.dy } else { work.bottom() - self.dy - size.height };
        keep_inside(Point { x, y }, size, work)
    }
}

/// The box the card is placed by, inside its window (logical pixels): the card's body (`layout`'s `card`, which the page
/// keeps where it rests while captions, a list or the avatar's dock grow the window around it) with the shadow margin
/// around it, so a card against an edge has its margin there, as before. Without a body, the whole window.
pub fn anchor(window: Size, body: Option<Rect>, margin: f64) -> Rect {
    let whole = Rect { x: 0.0, y: 0.0, width: window.width, height: window.height };
    let Some(b) = body.filter(|b| b.width > 0.0 && b.height > 0.0) else { return whole };
    let x = (b.x - margin).clamp(0.0, window.width);
    let y = (b.y - margin).clamp(0.0, window.height);
    Rect {
        x,
        y,
        width: (b.width + 2.0 * margin).min(window.width - x).max(1.0),
        height: (b.height + 2.0 * margin).min(window.height - y).max(1.0),
    }
}

/// Where the card's window goes: its body where it rests (`rest`, the anchor's top-left corner), the window around it,
/// shifted just enough to fit the work area when what grew around the body does not fit there.
pub fn window_at(rest: Point, anchor: Rect, window: Size, work: Rect) -> Point {
    fit(Point { x: rest.x - anchor.x, y: rest.y - anchor.y }, window, work)
}

/// `at`, moved just enough for a card of `size` to be inside the work area.
pub fn keep_inside(at: Point, size: Size, work: Rect) -> Point {
    Point {
        x: at.x.clamp(work.x, (work.right() - size.width).max(work.x)),
        y: at.y.clamp(work.y, (work.bottom() - size.height).max(work.y)),
    }
}

/// Whether the card is on screen (sidevoice-desktop#165): switched on in Settings, during a call, unless hidden for now
/// (the card's ×, the tray's "hide"), while the room's stage (the conversation's creature and its balloons) is not in
/// view ([`stage_in_view`]); with the stage in view only when the person chose it always shown
/// (`host.callControls.setAlwaysShown`). Which app has the focus never decides it.
pub fn wanted(enabled: bool, joined: bool, hidden: bool, stage_in_view: bool, always_shown: bool) -> bool {
    enabled && joined && !hidden && (always_shown || !stage_in_view)
}

/// Whether the person can see the room's stage: the system shows it ([`stage_on_screen`]) and the page shows it (not
/// Settings or another screen of the app that has no stage). A page that never said (a web build before #165) counts as
/// showing it, so an older web never brings the card up by itself.
pub fn stage_in_view(on_screen: bool, page_shows_it: Option<bool>) -> bool {
    on_screen && page_shows_it.unwrap_or(true)
}

/// How much of the stage must be left uncovered by other apps' windows for the person to see it: less than this, and it
/// counts as covered.
pub const STAGE_SEEN: f64 = 0.25;

/// Whether the system shows the room's stage: its window shown, not minimised, on screen (macOS: not fully covered nor
/// on another Space; Windows: not on another virtual desktop) and, where the app can measure it (macOS, Windows), at least
/// [`STAGE_SEEN`] of the stage not under other apps' windows (`uncovered`, [`uncovered`]).
pub fn stage_on_screen(shown: bool, minimised: bool, on_screen: bool, uncovered: Option<f64>) -> bool {
    shown && !minimised && on_screen && uncovered.is_none_or(|part| part >= STAGE_SEEN)
}

/// The part (0 to 1) of `target` that none of `above` covers. Each covered piece is counted once, however many windows
/// lie over it. An empty target is not seen at all.
pub fn uncovered(target: Rect, above: &[Rect]) -> f64 {
    let area = target.width.max(0.0) * target.height.max(0.0);
    if area <= 0.0 {
        return 0.0;
    }
    // The windows' parts over the target, cut to it.
    let over: Vec<Rect> = above
        .iter()
        .filter_map(|r| {
            let (x0, y0) = (r.x.max(target.x), r.y.max(target.y));
            let (x1, y1) = (r.right().min(target.right()), r.bottom().min(target.bottom()));
            (x1 > x0 && y1 > y0).then_some(Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 })
        })
        .collect();
    // The target cut on every edge of those parts: each cell is wholly covered or wholly not.
    let edges = |a: fn(&Rect) -> f64, b: fn(&Rect) -> f64, lo: f64, hi: f64| {
        let mut e: Vec<f64> = over.iter().flat_map(|r| [a(r), b(r)]).chain([lo, hi]).collect();
        e.sort_by(f64::total_cmp);
        e.dedup();
        e
    };
    let xs = edges(|r| r.x, Rect::right, target.x, target.right());
    let ys = edges(|r| r.y, Rect::bottom, target.y, target.bottom());
    let mut covered = 0.0;
    for x in xs.windows(2) {
        for y in ys.windows(2) {
            let middle = Point { x: (x[0] + x[1]) / 2.0, y: (y[0] + y[1]) / 2.0 };
            if over.iter().any(|r| r.contains(middle)) {
                covered += (x[1] - x[0]) * (y[1] - y[0]);
            }
        }
    }
    (1.0 - covered / area).clamp(0.0, 1.0)
}

/// The reason [`wanted`] answers as it does, for the app log: a card that is not there says why.
pub fn why(enabled: bool, joined: bool, hidden: bool, stage_in_view: bool, always_shown: bool) -> &'static str {
    if !enabled {
        "switched off"
    } else if !joined {
        "no call"
    } else if hidden {
        "hidden by the person"
    } else if always_shown {
        "always shown"
    } else if stage_in_view {
        "stage in view"
    } else {
        "stage not in view"
    }
}

/// Whether a card hidden for now (its ×, the tray's "hide") stays hidden: until the call ends or the stage comes into
/// view or leaves it (the room opened again, minimised, covered, Settings opened or closed).
pub fn still_hidden(hidden: bool, joined: bool, was_in_view: Option<bool>, in_view: bool) -> bool {
    hidden && joined && was_in_view.is_none_or(|was| was == in_view)
}

/// Orders changes to the card's window that are decided on several threads and applied later, on the main thread
/// (sidevoice-desktop#170). Each decision takes the [`advance`](Latest::advance) number under the card's lock; the
/// window is touched only after the lock is released, and only if [`is`](Latest::is) says no later decision was made
/// meanwhile.
/// The card's lock is never held while the app waits on the main thread (a monitor or window query), which deadlocked
/// with the main thread waiting for the lock; and a stale "hide" still never lands after a later "show" (#161).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Latest(u64);

impl Latest {
    /// A new decision: the number its apply must still find to land.
    pub fn advance(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }

    /// Whether the decision numbered `n` is still the latest one.
    pub fn is(&self, n: u64) -> bool {
        self.0 == n
    }
}

/// Whether the app must set the hand cursor itself now (macOS, #166): the pointer is over the card, the page says it is
/// over something clickable, and the cursor is not the hand already. Another app in front keeps setting its own cursor
/// (a text editor's I-beam) and WebKit sets the card's only when the pointer moves, so the app sets it on every look.
pub fn must_set_hand(pointer_inside: Option<bool>, page_wants_hand: bool, hand_now: bool) -> bool {
    pointer_inside == Some(true) && page_wants_hand && !hand_now
}

/// Where the card lands when dropped at `at` with its resting `size`: inside the work area, and against an edge or a
/// corner when it was dropped within [`MAGNET`] of it; otherwise exactly where it was dropped.
pub fn drop_at(at: Point, size: Size, work: Rect) -> Point {
    let (min_x, max_x) = (work.x, (work.right() - size.width).max(work.x));
    let (min_y, max_y) = (work.y, (work.bottom() - size.height).max(work.y));
    let x = at.x.clamp(min_x, max_x);
    let y = at.y.clamp(min_y, max_y);
    let snap = |v: f64, lo: f64, hi: f64| {
        if v - lo < MAGNET {
            lo
        } else if hi - v < MAGNET {
            hi
        } else {
            v
        }
    };
    Point { x: snap(x, min_x, max_x), y: snap(y, min_y, max_y) }
}

/// Where the card's window goes while it is `size`: where it rests when it fits there, else moved in just enough to
/// fit the work area; it returns when it shrinks again.
pub fn fit(rest: Point, size: Size, work: Rect) -> Point {
    let x = rest.x.min(work.right() - size.width).max(work.x);
    let y = rest.y.min(work.bottom() - size.height).max(work.y);
    Point { x, y }
}

/// The corner of the picture-in-picture the person resizes it by (its grip, toward the middle of the screen).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Nw,
    Ne,
    Sw,
    Se,
}

/// The picture's window (`start`, on its display) resized by its `edge` corner, the pointer `dx`, `dy` (logical pixels)
/// from where the resize started: the opposite corner stays where it is, each side is at least the page's least
/// (`min_width`, `min_height`) and the window stays within the work area. Free in both directions: the window takes any
/// shape, and the page fits its 16:9 stage inside it.
pub fn resized(start: Rect, edge: Edge, dx: f64, dy: f64, min_width: f64, min_height: f64, work: Rect) -> Rect {
    let west = matches!(edge, Edge::Nw | Edge::Sw);
    let north = matches!(edge, Edge::Nw | Edge::Ne);
    let room_x = if west { start.right() - work.x } else { work.right() - start.x };
    let room_y = if north { start.bottom() - work.y } else { work.bottom() - start.y };
    let width = (if west { start.width - dx } else { start.width + dx }).clamp(min_width, room_x.max(min_width));
    let height = (if north { start.height - dy } else { start.height + dy }).clamp(min_height, room_y.max(min_height));
    Rect {
        x: if west { start.right() - width } else { start.x },
        y: if north { start.bottom() - height } else { start.y },
        width,
        height,
    }
}

/// Where the card rests on each display, and the picture's size there when the person resized it, by the display's
/// key, so a laptop alone and at its desk keep their own.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Placements {
    #[serde(default)]
    pub displays: BTreeMap<String, Spot>,
    /// The picture-in-picture's window (its margin included) as last resized: the card's "Custom" size.
    #[serde(default)]
    pub sizes: BTreeMap<String, Size>,
}

pub const PLACEMENTS_FILE: &str = "call-controls.json";

impl Placements {
    /// What was stored, or nothing (a missing or unreadable file is a first run, never an error).
    pub fn load(dir: &Path) -> Self {
        fs::read_to_string(dir.join(PLACEMENTS_FILE))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{PLACEMENTS_FILE}.tmp"));
        fs::write(&tmp, serde_json::to_vec_pretty(self).expect("placements serialise"))?;
        fs::rename(tmp, dir.join(PLACEMENTS_FILE))
    }

    /// Whether the person moved or resized the card anywhere: "Reset size and position" has something to forget.
    pub fn moved(&self) -> bool {
        !self.displays.is_empty() || !self.sizes.is_empty()
    }
}

/// The card advertises only a shortcut the system actually registered. The saved chord alone is no guarantee.
pub fn shortcut_hint(shortcut: &crate::settings::MuteShortcut) -> &str {
    if shortcut.state == crate::settings::Registration::On {
        shortcut.label.as_deref().unwrap_or_default()
    } else {
        ""
    }
}

/// A Tauri accelerator as the card's tooltip reads it: macOS's symbols (⌃⌥⇧⌘) run together, elsewhere the names joined
/// with "+". Empty stays empty (no shortcut).
pub fn shortcut_label(accelerator: &str, macos: bool) -> String {
    let parts: Vec<&str> = accelerator.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return String::new();
    }
    let name = |part: &str| -> String {
        let lower = part.to_ascii_lowercase();
        let modifier = match lower.as_str() {
            "ctrl" | "control" => Some(("⌃", "Ctrl")),
            "alt" | "option" => Some(("⌥", "Alt")),
            "shift" => Some(("⇧", "Shift")),
            "cmd" | "command" | "super" | "meta" => Some(("⌘", if macos { "Cmd" } else { "Super" })),
            "cmdorctrl" | "commandorcontrol" => Some(if macos { ("⌘", "Cmd") } else { ("⌃", "Ctrl") }),
            _ => None,
        };
        match modifier {
            Some((symbol, word)) => (if macos { symbol } else { word }).to_string(),
            None => {
                let key = part.strip_prefix("Key").filter(|k| k.len() == 1).unwrap_or(part);
                key.to_uppercase()
            }
        }
    };
    let names: Vec<String> = parts.iter().map(|p| name(p)).collect();
    if macos {
        names.concat()
    } else {
        names.join("+")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect { x: 0.0, y: 25.0, width: 1440.0, height: 850.0 }; // a MacBook under its menu bar
    const CARD: Size = Size { width: 414.0, height: 82.0 };

    /// The whole rule (#165), every case: the card shows when switched on, in a call, not hidden for now, and either
    /// always shown or with the stage out of view, on screen or in the page.
    #[test]
    fn shortcut_hint_requires_successful_registration() {
        use crate::settings::{MuteShortcut, Registration};
        for macos in [false, true] {
            let on = MuteShortcut::new("Ctrl+Alt+M", Registration::On, vec![], macos);
            assert_eq!(shortcut_hint(&on), on.label.as_deref().unwrap());
            for state in [Registration::Off, Registration::Refused, Registration::Unavailable] {
                let kept = MuteShortcut::new("Ctrl+Alt+M", state, vec![], macos);
                assert!(kept.label.is_some(), "a saved chord remains configured");
                assert_eq!(shortcut_hint(&kept), "", "no working shortcut for {state:?}");
            }
        }
        assert_eq!(shortcut_hint(&MuteShortcut::default()), "");
    }

    #[test]
    fn the_card_shows_exactly_when_the_stage_is_out_of_view() {
        for bits in 0..64u8 {
            let [enabled, joined, hidden, on_screen, always] = [0, 1, 2, 3, 4].map(|i| bits & (1 << i) != 0);
            for page in [None, Some(true), Some(false)] {
                let in_view = stage_in_view(on_screen, page);
                let shown = wanted(enabled, joined, hidden, in_view, always);
                let page_shows = page != Some(false);
                let expected = enabled && joined && !hidden && (always || !(on_screen && page_shows));
                assert_eq!(shown, expected, "{bits:05b} page={page:?}");
            }
        }
    }

    #[test]
    fn the_cases_the_operator_named() {
        let card = |on_screen: bool, page: Option<bool>, enabled: bool| {
            wanted(enabled, true, false, stage_in_view(on_screen, page), false)
        };
        // The room in front with its stage, even with another app focused (focus is not an input at all): no card.
        assert!(!card(true, Some(true), true));
        // The room hidden, minimised, covered, on another Space: the card.
        assert!(card(false, Some(true), true));
        // Settings (or another screen of the app without the stage), the room in front: the card.
        assert!(card(true, Some(false), true));
        // Settings closed again: no card.
        assert!(!card(true, Some(true), true));
        // A web before #165 never says: the system alone decides, and an older page never brings the card up.
        assert!(!card(true, None, true));
        assert!(card(false, None, true));
        // Switched off: never.
        for (on_screen, page) in [(false, Some(true)), (true, Some(false)), (false, None), (false, Some(false))] {
            assert!(!card(on_screen, page, false), "{on_screen} {page:?}");
        }
    }

    #[test]
    fn the_system_shows_the_stage_while_its_window_is_shown_and_enough_of_it_uncovered() {
        assert!(stage_on_screen(true, false, true, None), "nothing measured (Windows, Linux): the window decides");
        assert!(stage_on_screen(true, false, true, Some(1.0)));
        assert!(stage_on_screen(true, false, true, Some(STAGE_SEEN)), "a quarter is enough to see it");
        assert!(!stage_on_screen(true, false, true, Some(0.1)), "covered all but a sliver");
        assert!(!stage_on_screen(true, true, true, Some(1.0)), "minimised");
        assert!(!stage_on_screen(false, false, true, None), "hidden (closed to the tray)");
        assert!(!stage_on_screen(true, false, false, None), "fully covered or on another Space");
    }

    #[test]
    fn the_uncovered_part_counts_each_covered_piece_once() {
        let stage = Rect { x: 100.0, y: 100.0, width: 400.0, height: 200.0 };
        assert_eq!(uncovered(stage, &[]), 1.0);
        let left_half = Rect { x: 0.0, y: 0.0, width: 300.0, height: 1000.0 };
        assert_eq!(uncovered(stage, &[left_half]), 0.5);
        // The same half under two windows is still half.
        assert_eq!(uncovered(stage, &[left_half, Rect { x: 150.0, y: 120.0, width: 100.0, height: 50.0 }]), 0.5);
        let right_quarter = Rect { x: 400.0, y: 0.0, width: 1000.0, height: 1000.0 };
        assert_eq!(uncovered(stage, &[left_half, right_quarter]), 0.25);
        assert_eq!(uncovered(stage, &[Rect { x: 0.0, y: 0.0, width: 2000.0, height: 2000.0 }]), 0.0);
        assert_eq!(uncovered(stage, &[Rect { x: 600.0, y: 0.0, width: 100.0, height: 100.0 }]), 1.0, "beside it");
        assert_eq!(uncovered(Rect { width: 0.0, ..stage }, &[]), 0.0, "no stage, nothing seen");
    }

    #[test]
    fn the_hand_is_set_again_only_over_the_card_on_something_clickable_and_not_already_the_cursor() {
        assert!(must_set_hand(Some(true), true, false), "another app's I-beam is there: take it back");
        assert!(!must_set_hand(Some(true), true, true), "already the hand");
        assert!(!must_set_hand(Some(true), false, false), "over the card's body: the arrow is right");
        assert!(!must_set_hand(Some(false), true, false), "the pointer is elsewhere: never touch another app's cursor");
        assert!(!must_set_hand(None, true, false), "pointer unknown");
    }

    #[test]
    fn the_reason_always_agrees_with_the_answer() {
        for bits in 0..32u8 {
            let [enabled, joined, hidden, in_view, always] = [0, 1, 2, 3, 4].map(|i| bits & (1 << i) != 0);
            let reason = why(enabled, joined, hidden, in_view, always);
            let shown = wanted(enabled, joined, hidden, in_view, always);
            assert_eq!(shown, matches!(reason, "always shown" | "stage not in view"), "{bits:05b}: {reason}");
        }
    }

    #[test]
    fn hidden_by_its_x_until_the_stage_comes_into_view_or_leaves_or_the_call_ends() {
        assert!(still_hidden(true, true, Some(false), false), "the room still minimised: still hidden");
        assert!(still_hidden(true, true, None, false), "nothing seen of the room yet");
        assert!(!still_hidden(true, true, Some(false), true), "the room opened again, Settings closed");
        assert!(!still_hidden(true, true, Some(true), false), "the room minimised, Settings opened");
        assert!(!still_hidden(true, false, Some(false), false), "the next call shows it again");
        assert!(!still_hidden(false, true, Some(false), false));
    }

    #[test]
    fn first_placed_in_the_chosen_corner() {
        assert_eq!(Spot::corner(Corner::TopRight).at(CARD, WORK), Point { x: 1026.0, y: 25.0 });
        assert_eq!(Spot::corner(Corner::BottomLeft).at(CARD, WORK), Point { x: 0.0, y: 793.0 });
        assert_eq!(Corner::default(), Corner::TopRight);
        assert_eq!(serde_json::to_string(&Corner::BottomRight).unwrap(), r#""br""#);
        assert_eq!(Corner::BottomLeft.place(), serde_json::json!({"vertical": "lower", "horizontal": "left"}));
    }

    #[test]
    fn a_card_against_an_edge_stays_against_it_at_any_size() {
        let spot = Spot::of(drop_at(Point { x: 1400.0, y: 400.0 }, CARD, WORK), CARD, WORK);
        assert_eq!((spot.corner, spot.dx), (Corner::TopRight, 0.0), "snapped to the right edge, upper half");
        let avatar = Size { width: 184.0, height: 184.0 };
        assert_eq!(spot.at(avatar, WORK).x, 1440.0 - 184.0, "the avatar is against the right edge too");
        let low = Spot::of(Point { x: 300.0, y: 600.0 }, CARD, WORK);
        assert_eq!(low.corner, Corner::BottomLeft);
        let grown = Size { width: 414.0, height: 200.0 };
        assert_eq!(low.at(grown, WORK).y + grown.height, 600.0 + CARD.height, "grows up, away from its corner");
        assert_eq!(Spot::of(Point { x: 600.0, y: 300.0 }, CARD, WORK).at(CARD, WORK), Point { x: 600.0, y: 300.0 });
    }

    #[test]
    fn dropped_near_an_edge_it_snaps_to_it_else_it_stays() {
        let card = Size { width: 344.0, height: 82.0 };
        assert_eq!(drop_at(Point { x: 600.0, y: 400.0 }, card, WORK), Point { x: 600.0, y: 400.0 });
        assert_eq!(drop_at(Point { x: 30.0, y: 400.0 }, card, WORK), Point { x: 0.0, y: 400.0 });
        assert_eq!(drop_at(Point { x: 1080.0, y: 780.0 }, card, WORK), Point { x: 1096.0, y: 793.0 });
        assert_eq!(drop_at(Point { x: -200.0, y: -50.0 }, card, WORK), Point { x: 0.0, y: 25.0 });
    }

    #[test]
    fn the_body_stays_still_while_the_window_grows_around_it() {
        // The title card in the lower half with its captions open above it: the body (12 px margin) is 160 px down.
        let window = Size { width: 414.0, height: 260.0 };
        let body = Rect { x: 12.0, y: 172.0, width: 390.0, height: 76.0 };
        let a = anchor(window, Some(body), 12.0);
        assert_eq!(a, Rect { x: 0.0, y: 160.0, width: 414.0, height: 100.0 });
        let rest = Point { x: 600.0, y: 500.0 };
        assert_eq!(window_at(rest, a, window, WORK), Point { x: 600.0, y: 340.0 }, "the window around the body");
        // At the top of the work area there is no room above: the window moves just enough, the rest place stays.
        let top = Point { x: 600.0, y: 60.0 };
        assert_eq!(window_at(top, a, window, WORK), Point { x: 600.0, y: 25.0 });
        // No body: the whole window, as before.
        assert_eq!(anchor(window, None, 12.0), Rect { x: 0.0, y: 0.0, width: 414.0, height: 260.0 });
    }

    #[test]
    fn the_picture_resizes_freely_from_its_grip_keeping_the_far_corner() {
        // At the top right; its grip in the bottom-left corner.
        let start = Rect { x: 996.0, y: 25.0, width: 444.0, height: 400.0 };
        let wider = resized(start, Edge::Sw, -160.0, 0.0, 374.0, 354.0, WORK);
        assert_eq!(
            wider,
            Rect { x: 836.0, y: 25.0, width: 604.0, height: 400.0 },
            "top-right stays, the height is its own"
        );
        let taller = resized(start, Edge::Sw, 0.0, 200.0, 374.0, 354.0, WORK);
        assert_eq!(taller, Rect { x: 996.0, y: 25.0, width: 444.0, height: 600.0 }, "the aspect changes");
        let narrow = resized(start, Edge::Sw, 400.0, -400.0, 374.0, 354.0, WORK);
        assert_eq!((narrow.width, narrow.height), (374.0, 354.0), "never under either minimum");
        assert_eq!(narrow.x + narrow.width, start.x + start.width);
        let huge = resized(start, Edge::Sw, -5000.0, 5000.0, 374.0, 354.0, WORK);
        assert!(huge.x >= WORK.x && huge.y + huge.height <= WORK.y + WORK.height + 1e-9, "within the work area");
        let low = Rect { x: 500.0, y: 400.0, width: 444.0, height: 400.0 };
        let up = resized(low, Edge::Ne, 100.0, -100.0, 374.0, 354.0, WORK);
        assert_eq!(
            up,
            Rect { x: 500.0, y: 300.0, width: 544.0, height: 500.0 },
            "north grows upward, its bottom edge still"
        );
    }

    #[test]
    fn each_display_is_its_own_space() {
        // Windows / X11: a 100% display at the desktop's origin and a 200% one to its right, in physical pixels.
        let right = Display::new("Built-in", 2.0, Point { x: 1920.0, y: 50.0 }, Size { width: 1440.0, height: 850.0 });
        let p = Point { x: 100.0, y: 40.0 };
        assert_eq!(right.to_desktop(p), Point { x: 2120.0, y: 130.0 });
        assert_eq!(right.to_local(Point { x: 2120.0, y: 130.0 }), p);
        // macOS: the desktop is in points, so a display's unit is 1 whatever its scale.
        let mac = Display::new("Built-in", 1.0, Point { x: -1440.0, y: 25.0 }, Size { width: 1440.0, height: 875.0 });
        assert_eq!(mac.to_desktop(p), Point { x: -1340.0, y: 65.0 });
        // Two displays of one model keep their own places.
        let size = Size { width: 10.0, height: 10.0 };
        assert_ne!(
            Display::new("DELL", 1.0, Point { x: 0.0, y: 0.0 }, size).key,
            Display::new("DELL", 1.0, Point { x: 1920.0, y: 0.0 }, size).key
        );
    }

    #[test]
    fn placements_and_sizes_are_remembered_per_display() {
        let dir = std::env::temp_dir().join(format!("sidevoice-call-controls-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(Placements::load(&dir), Placements::default(), "nothing stored yet");
        assert!(!Placements::default().moved());
        let mut placements = Placements::default();
        placements
            .displays
            .insert("Built-in Retina Display".into(), Spot { corner: Corner::BottomLeft, dx: 10.0, dy: 30.0 });
        placements.sizes.insert("Built-in Retina Display".into(), Size { width: 500.0, height: 400.0 });
        assert!(placements.moved());
        placements.save(&dir).unwrap();
        assert_eq!(Placements::load(&dir), placements);
        fs::write(dir.join(PLACEMENTS_FILE), "{ broken").unwrap();
        assert_eq!(Placements::load(&dir), Placements::default(), "unreadable: start over");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// How the main thread applies decision `n`, wanting `visible`, to the window (`None`: never touched yet).
    type Apply = dyn Fn(&Latest, u64, bool, &mut Option<bool>);

    /// Every order in which the main thread may run the applies of `decided` (each queued after its own decision was
    /// taken, in any order between threads): the window ends as `apply` leaves it.
    fn every_order(decided: &[bool], apply: &Apply) -> Vec<Option<bool>> {
        fn walk(
            decided: &[bool],
            apply: &Apply,
            latest: Latest,
            taken: usize,
            pending: Vec<(u64, bool)>,
            window: Option<bool>,
            out: &mut Vec<Option<bool>>,
        ) {
            if taken == decided.len() && pending.is_empty() {
                out.push(window);
                return;
            }
            if taken < decided.len() {
                let mut latest = latest;
                let n = latest.advance();
                let mut pending = pending.clone();
                pending.push((n, decided[taken]));
                walk(decided, apply, latest, taken + 1, pending, window, out);
            }
            for i in 0..pending.len() {
                let mut rest = pending.clone();
                let (n, visible) = rest.remove(i);
                let mut window = window;
                apply(&latest, n, visible, &mut window);
                walk(decided, apply, latest, taken, rest, window, out);
            }
        }
        let mut out = Vec::new();
        walk(decided, apply, Latest::default(), 0, Vec::new(), None, &mut out);
        out
    }

    #[test]
    fn the_window_ends_as_the_last_decision_whatever_order_the_applies_run_in() {
        let checked = |latest: &Latest, n: u64, visible: bool, window: &mut Option<bool>| {
            if latest.is(n) {
                *window = Some(visible);
            }
        };
        for decided in [vec![true, false], vec![false, true], vec![true, false, true], vec![false, true, false, true]] {
            let last = *decided.last().unwrap();
            let ends = every_order(&decided, &checked);
            assert!(ends.len() > decided.len(), "interleavings explored for {decided:?}");
            assert!(ends.iter().all(|w| *w == Some(last)), "{decided:?}: the window agrees with the last decision");
        }
        // Without the check, a stale "hide" queued after a later "show" lands last: the card's state says shown and its
        // window is hidden, with no later change to correct it (#161).
        let unchecked = |_: &Latest, _: u64, visible: bool, window: &mut Option<bool>| *window = Some(visible);
        assert!(every_order(&[false, true], &unchecked).contains(&Some(false)));
    }

    /// The app's shape, with threads: deciders change the state under a lock and queue the apply to one "main thread",
    /// which takes the same lock to check it. No decider ever waits on the main thread with the lock held, so this
    /// cannot deadlock however the threads interleave; and the window ends as the state says.
    #[test]
    fn deciders_and_the_main_thread_share_the_lock_without_waiting_on_each_other() {
        use std::sync::{mpsc, Arc, Mutex};
        type Task = Box<dyn FnOnce() + Send>;
        for round in 0..50 {
            let state = Arc::new(Mutex::new((false, Latest::default())));
            let window = Arc::new(Mutex::new(false));
            let (main, tasks) = mpsc::channel::<Task>();
            let main_thread = std::thread::spawn(move || {
                for task in tasks {
                    task();
                }
            });
            let deciders: Vec<_> = (0..4)
                .map(|t| {
                    let (state, window, main) = (state.clone(), window.clone(), main.clone());
                    std::thread::spawn(move || {
                        for i in 0..50 {
                            let visible = (t + i + round) % 3 != 0;
                            let n = {
                                let mut s = state.lock().unwrap();
                                s.0 = visible;
                                s.1.advance()
                            };
                            let (state, window) = (state.clone(), window.clone());
                            main.send(Box::new(move || {
                                if state.lock().unwrap().1.is(n) {
                                    *window.lock().unwrap() = visible;
                                }
                            }))
                            .unwrap();
                        }
                    })
                })
                .collect();
            for d in deciders {
                d.join().unwrap();
            }
            drop(main);
            main_thread.join().unwrap();
            assert_eq!(*window.lock().unwrap(), state.lock().unwrap().0, "round {round}");
        }
    }

    #[test]
    fn the_shortcut_reads_as_each_system_writes_it() {
        assert_eq!(shortcut_label("Ctrl+Alt+M", true), "⌃⌥M");
        assert_eq!(shortcut_label("Ctrl+Alt+M", false), "Ctrl+Alt+M");
        assert_eq!(shortcut_label("CmdOrCtrl+Shift+KeyM", true), "⌘⇧M");
        assert_eq!(shortcut_label("CmdOrCtrl+Shift+m", false), "Ctrl+Shift+M");
        assert_eq!(shortcut_label("Alt+F9", false), "Alt+F9");
        assert_eq!(shortcut_label("", true), "");
    }
}
