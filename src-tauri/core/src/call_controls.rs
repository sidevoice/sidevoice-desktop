//! The call controls card's pure logic (sidevoice/sidevoice-desktop#4): when it shows, where it sits, how it moves
//! and grows, where it is remembered, and how the mute shortcut reads on it. The window itself is `src/call_controls.rs`.
//!
//! Geometry is in logical pixels of one display, relative to the top-left corner of its work area (y down), of the
//! card's *window*: the card plus the transparent margin its shadow lives in. Each display is its own space, so displays
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

#[derive(Debug, Clone, Copy, PartialEq)]
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

/// Where the card rests on a display: where it was left there (kept inside), or its first place. Always worked out from
/// the card at rest (`rest_size`), never from the card grown with its controls, so a card shown, grown, hidden and
/// shown again comes back to the same place.
pub fn resting_place(stored: Option<Point>, rest_size: Size, work: Rect) -> Point {
    drop_at(stored.unwrap_or_else(|| first_place(work, rest_size)), rest_size, work)
}

/// Whether the card is on screen: during a call, unless hidden for this call from the tray, and never while the room's
/// own window is in front and focused (the same controls are there).
pub fn wanted(joined: bool, hidden_for_call: bool, room_in_front: bool) -> bool {
    joined && !hidden_for_call && !room_in_front
}

/// Where a card that has never been placed on this display goes: the top-right corner of its work area.
pub fn first_place(work: Rect, size: Size) -> Point {
    Point { x: work.right() - size.width, y: work.y }
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

/// Where the card's window goes while it is `size` (resting, or grown with its controls and panels): at its resting
/// place when it fits there, else raised (and moved in) just enough to fit the work area. Controls always open below
/// the card; near the bottom it is the card that moves up, and it returns when it shrinks again.
pub fn fit(rest: Point, size: Size, work: Rect) -> Point {
    let x = rest.x.min(work.right() - size.width).max(work.x);
    let y = rest.y.min(work.bottom() - size.height).max(work.y);
    Point { x, y }
}

/// Where the card rests on each display, by the display's name, so a laptop alone and at its desk keep their own.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Placements {
    #[serde(default)]
    pub displays: BTreeMap<String, Point>,
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
    const CARD: Size = Size { width: 344.0, height: 82.0 };

    #[test]
    fn shown_during_a_call_unless_hidden_or_the_room_is_in_front() {
        assert!(wanted(true, false, false));
        assert!(!wanted(false, false, false), "no call, no card");
        assert!(!wanted(true, true, false), "hidden from the tray for this call");
        assert!(!wanted(true, false, true), "the room is in front: its controls are there");
    }

    #[test]
    fn first_placed_top_right() {
        assert_eq!(first_place(WORK, CARD), Point { x: 1096.0, y: 25.0 });
    }

    #[test]
    fn dropped_near_an_edge_it_snaps_to_it_else_it_stays() {
        // Free: exactly where dropped.
        assert_eq!(drop_at(Point { x: 600.0, y: 400.0 }, CARD, WORK), Point { x: 600.0, y: 400.0 });
        // Near the left edge only: snaps left, keeps its height.
        assert_eq!(drop_at(Point { x: 30.0, y: 400.0 }, CARD, WORK), Point { x: 0.0, y: 400.0 });
        // Near a corner: into the corner.
        assert_eq!(drop_at(Point { x: 1080.0, y: 780.0 }, CARD, WORK), Point { x: 1096.0, y: 793.0 });
        // Off the work area: brought back, against the edge.
        assert_eq!(drop_at(Point { x: -200.0, y: -50.0 }, CARD, WORK), Point { x: 0.0, y: 25.0 });
    }

    #[test]
    fn grown_near_the_bottom_it_rises_just_enough_and_returns() {
        let rest = Point { x: 1096.0, y: 793.0 }; // the bottom-right corner, resting
        let grown = Size { width: 344.0, height: 128.0 };
        assert_eq!(fit(rest, grown, WORK), Point { x: 1096.0, y: 747.0 }, "raised by what the controls add");
        assert_eq!(fit(rest, CARD, WORK), rest, "back where it rests");
        let top = Point { x: 600.0, y: 25.0 };
        assert_eq!(fit(top, grown, WORK), top, "with room below, nothing moves");
        let huge = Size { width: 344.0, height: 2000.0 };
        assert_eq!(fit(rest, huge, WORK).y, WORK.y, "never above the work area");
    }

    #[test]
    fn showing_it_again_grown_never_moves_where_it_rests() {
        let bottom = Point { x: 1096.0, y: 793.0 };
        let grown = Size { width: 344.0, height: 128.0 };
        // Shown grown (the pointer on it, or "always"), it is raised to fit; where it rests stays where it was.
        let rest = resting_place(Some(bottom), CARD, WORK);
        assert_eq!(rest, bottom);
        assert_eq!(fit(rest, grown, WORK).y, 747.0);
        assert_eq!(resting_place(None, CARD, WORK), first_place(WORK, CARD));
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
    fn placements_are_remembered_per_display() {
        let dir = std::env::temp_dir().join(format!("sidevoice-call-controls-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(Placements::load(&dir), Placements::default(), "nothing stored yet");
        let mut placements = Placements::default();
        placements.displays.insert("Built-in Retina Display".into(), Point { x: 10.0, y: 30.0 });
        placements.save(&dir).unwrap();
        assert_eq!(Placements::load(&dir), placements);
        fs::write(dir.join(PLACEMENTS_FILE), "{ broken").unwrap();
        assert_eq!(Placements::load(&dir), Placements::default(), "unreadable: start over");
        fs::remove_dir_all(&dir).unwrap();
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
