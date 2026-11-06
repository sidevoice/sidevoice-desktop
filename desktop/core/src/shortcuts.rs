//! The global shortcuts, as pure logic (docs/SHORTCUTS.md, docs/BRIDGE.md → "The shortcuts"): the three actions, how an
//! accelerator is written and read (a combination, or a single key with how it is pressed), what a gesture does, how
//! the registrations follow the settings, the safety net for a hold that never ends, and what the page's recorder
//! captures. No Tauri and no operating system here: the shell feeds it events and acts on what it answers, so every rule
//! is unit-tested on any machine.

use crate::settings::{self, GlobalShortcut, Registration};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant, SystemTime};

/// A tap, and the gap between the two taps of a double tap: under this.
pub const TAP_WINDOW: Duration = Duration::from_millis(400);
/// On a key that also carries a tap or a double, a press is a hold once it has lasted this long (and a tap is shorter).
pub const HOLD_AFTER: Duration = Duration::from_millis(200);
/// A hold with no release for this long is let go (the safety net: a missed `Released` must not leave the microphone open).
pub const STUCK_HOLD: Duration = Duration::from_secs(60);
/// How often a hold is looked at while it lasts.
pub const WATCH_TICK: Duration = Duration::from_secs(1);
/// A pause between two looks longer than this was a sleep: the machine was not running.
pub const SLEEP_GAP: Duration = Duration::from_secs(5);

/// Ctrl+Alt+M on macOS (⌃⌥M), as it always was there.
pub const DEFAULT_MUTE_MACOS: &str = "Ctrl+Alt+M";
/// Ctrl+Alt+Shift+M on Windows and Linux: Ctrl+Alt+letter is AltGr on many layouts, and Ctrl+Shift+M is Teams' and Discord's.
pub const DEFAULT_MUTE_OTHER: &str = "Ctrl+Alt+Shift+M";
/// The mute shortcut every build before this one registered on every system. A person still on it was never asked.
pub const LEGACY_MUTE_SHORTCUT: &str = "Ctrl+Alt+M";

/// The default of `action` on a system whose default mute shortcut is `mute`: hold to talk and say one thing have none (empty).
pub fn default_of(action: Action, mute: &str) -> &str {
    match action {
        Action::Mute => mute,
        Action::HoldToTalk | Action::OneTurn => "",
    }
}

/// What a global shortcut does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    /// Mute or unmute the microphone (the page's `toggleMic`).
    Mute,
    /// Talk while it is held, and while the microphone is muted (the page's `holdStart` and `holdEnd`).
    HoldToTalk,
    /// Say one thing: the microphone opens for one turn (the page's `oneTurn`).
    OneTurn,
}

impl Action {
    /// In the order the app keeps them, and the order that wins when two hold the same keys.
    pub const ALL: [Action; 3] = [Action::Mute, Action::HoldToTalk, Action::OneTurn];

    pub fn index(self) -> usize {
        match self {
            Action::Mute => 0,
            Action::HoldToTalk => 1,
            Action::OneTurn => 2,
        }
    }

    /// The name the page writes it by.
    pub fn name(self) -> &'static str {
        match self {
            Action::Mute => "mute",
            Action::HoldToTalk => "holdToTalk",
            Action::OneTurn => "oneTurn",
        }
    }

    pub fn parse(name: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.name() == name)
    }
}

/// How a single key is pressed to act.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// While it is down.
    Hold,
    /// Pressed and let go within [`TAP_WINDOW`], with no other key between.
    Tap,
    /// Two taps, the second within [`TAP_WINDOW`] of the first.
    Double,
}

impl Trigger {
    fn name(self) -> &'static str {
        match self {
            Trigger::Hold => "hold",
            Trigger::Tap => "tap",
            Trigger::Double => "double",
        }
    }

    fn parse(name: &str) -> Option<Trigger> {
        [Trigger::Hold, Trigger::Tap, Trigger::Double].into_iter().find(|t| t.name().eq_ignore_ascii_case(name))
    }
}

/// A key pressed on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleKey {
    /// The Fn (Globe) key of a Mac.
    Fn,
    RightAlt,
    RightCommand,
    RightControl,
    RightShift,
    /// F1 to F24: a key a system registers as a hot key, with no listener and no permission.
    Function(u8),
}

impl SingleKey {
    pub fn parse(name: &str) -> Option<SingleKey> {
        match name {
            "Fn" => Some(SingleKey::Fn),
            "RightAlt" => Some(SingleKey::RightAlt),
            "RightCommand" => Some(SingleKey::RightCommand),
            "RightControl" => Some(SingleKey::RightControl),
            "RightShift" => Some(SingleKey::RightShift),
            _ => function_key(name).map(SingleKey::Function),
        }
    }

    pub fn name(self) -> String {
        match self {
            SingleKey::Fn => "Fn".into(),
            SingleKey::RightAlt => "RightAlt".into(),
            SingleKey::RightCommand => "RightCommand".into(),
            SingleKey::RightControl => "RightControl".into(),
            SingleKey::RightShift => "RightShift".into(),
            SingleKey::Function(n) => format!("F{n}"),
        }
    }

    /// Whether only a listener of every key can see it (a modifier, Fn): the system gives no hot key for these.
    pub fn needs_listener(self) -> bool {
        !matches!(self, SingleKey::Function(_))
    }

    /// Whether the key is down in the flags a macOS event carries (`CGEventFlags`, `NSEventModifierFlags`): the device-side
    /// bits tell the right-hand modifiers from the left-hand ones, `kCGEventFlagMaskSecondaryFn` the Fn key.
    pub fn macos_down(self, flags: u64) -> bool {
        let mask = match self {
            SingleKey::Fn => 0x0080_0000,
            SingleKey::RightShift => 0x0000_0004,
            SingleKey::RightCommand => 0x0000_0010,
            SingleKey::RightAlt => 0x0000_0040,
            SingleKey::RightControl => 0x0000_2000,
            SingleKey::Function(_) => return false,
        };
        flags & mask != 0
    }

    /// Whether the left-hand modifier a macOS virtual key code is (`kVK_Command`, `kVK_Shift`, `kVK_Option`, `kVK_Control`) is down
    /// in the flags a `flagsChanged` event carries, by the device-side bit of that side; `None` for any other key.
    pub fn left_macos_down(code: u16, flags: u64) -> Option<bool> {
        let mask = match code {
            55 => 0x0000_0008,
            56 => 0x0000_0002,
            58 => 0x0000_0020,
            59 => 0x0000_0001,
            _ => return None,
        };
        Some(flags & mask != 0)
    }

    /// The single key a macOS virtual key code is, as a `flagsChanged` event reports it (`kVK_Function`, the right-hand modifiers).
    pub fn from_macos_modifier(code: u16) -> Option<SingleKey> {
        match code {
            63 => Some(SingleKey::Fn),
            61 => Some(SingleKey::RightAlt),
            54 => Some(SingleKey::RightCommand),
            62 => Some(SingleKey::RightControl),
            60 => Some(SingleKey::RightShift),
            _ => None,
        }
    }
}

/// `F1` to `F24`.
fn function_key(name: &str) -> Option<u8> {
    let digits = name.strip_prefix('F')?;
    if digits.is_empty() || digits.len() > 2 || !digits.bytes().all(|b| b.is_ascii_digit()) || digits.starts_with('0') {
        return None;
    }
    digits.parse().ok().filter(|n| (1..=24).contains(n))
}

/// What an accelerator asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// Keys pressed together, as [`settings::normalize_shortcut`] writes them (`Ctrl+Alt+M`, `F9`).
    Combo(String),
    /// A key on its own and how it is pressed (`Fn:hold`, `RightControl:double`, `F13:tap`).
    Single { key: SingleKey, trigger: Trigger },
}

impl Parsed {
    /// The one form the app keeps and the page is told.
    pub fn accelerator(&self) -> String {
        match self {
            Parsed::Combo(keys) => keys.clone(),
            Parsed::Single { key, trigger } => format!("{}:{}", key.name(), trigger.name()),
        }
    }

    /// The hot key a system registers for it, when it has one: a combination, or an F-key on its own.
    pub fn hot_key(&self) -> Option<String> {
        match self {
            Parsed::Combo(keys) => Some(keys.clone()),
            Parsed::Single { key: SingleKey::Function(n), .. } => Some(format!("F{n}")),
            Parsed::Single { .. } => None,
        }
    }

    /// The key a listener must see, when no hot key can carry it.
    pub fn listened(&self) -> Option<SingleKey> {
        match self {
            Parsed::Single { key, .. } if key.needs_listener() => Some(*key),
            _ => None,
        }
    }

    pub fn is_single(&self) -> bool {
        matches!(self, Parsed::Single { .. })
    }
}

/// An accelerator as the page writes it (empty: none). `None`: not one the app can keep.
pub fn parse(text: &str) -> Option<Option<Parsed>> {
    let text = text.trim();
    if text.is_empty() {
        return Some(None);
    }
    if let Some((body, suffix)) = text.rsplit_once(':') {
        if let Some(trigger) = Trigger::parse(suffix) {
            return SingleKey::parse(body.trim()).map(|key| Some(Parsed::Single { key, trigger }));
        }
    }
    settings::normalize_shortcut(text).map(|keys| Some(Parsed::Combo(keys)))
}

/// Whether `action` can be carried out by what was asked: hold to talk is held (a combination, or a key held); the others
/// are done once (a combination, or a key tapped or double tapped).
pub fn suits(action: Action, parsed: &Parsed) -> bool {
    match (action, parsed) {
        (_, Parsed::Combo(_)) => true,
        (Action::HoldToTalk, Parsed::Single { trigger, .. }) => *trigger == Trigger::Hold,
        (Action::Mute | Action::OneTurn, Parsed::Single { trigger, .. }) => *trigger != Trigger::Hold,
    }
}

/// The accelerator as the card's tooltip and the page read it on this system (symbols on macOS). A single key reads as its
/// name, a right-hand one with its side's word from the app's own texts, and a double tap with "×2".
pub fn label(accelerator: &str, macos: bool, language: &str) -> String {
    match parse(accelerator) {
        Some(Some(Parsed::Single { key, trigger })) => {
            let modifier = |symbol: &str, name: &str| {
                format!("{} {}", crate::i18n::t(language, "shortcut.right"), if macos { symbol } else { name })
            };
            let name = match key {
                SingleKey::Fn => "Fn".to_string(),
                SingleKey::RightAlt => modifier("⌥", "Alt"),
                SingleKey::RightCommand => modifier("⌘", if macos { "Cmd" } else { "Win" }),
                SingleKey::RightControl => modifier("⌃", "Ctrl"),
                SingleKey::RightShift => modifier("⇧", "Shift"),
                SingleKey::Function(n) => format!("F{n}"),
            };
            if trigger == Trigger::Double {
                format!("{name} ×2")
            } else {
                name
            }
        }
        _ => crate::call_controls::shortcut_label(accelerator, macos),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// What a gesture does
// ---------------------------------------------------------------------------------------------------------------------

/// What a combination does when it is pressed. A single key is a [`KeyMachine]'s, not a gesture of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// At once, when it goes down: a combination of mute or one turn.
    Press,
    Hold,
}

/// The gesture a combination makes of `action`: hold to talk is held, any other acts when pressed. `None` for a single key.
pub fn gesture(action: Action, parsed: &Parsed) -> Option<Gesture> {
    match (action, parsed) {
        (Action::HoldToTalk, Parsed::Combo(_)) => Some(Gesture::Hold),
        (_, Parsed::Combo(_)) => Some(Gesture::Press),
        (_, Parsed::Single { .. }) => None,
    }
}

/// What the key's own events are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Down,
    Up,
    /// Another key was pressed: what is pending was not a lone key.
    Other,
}

/// What a listener of the keyboard hears, and no more: which of the app's single keys went down or up, and that another key was
/// pressed (so a pending tap was a modifier of it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listened {
    Single { key: SingleKey, down: bool },
    Other,
}

/// What a listener's event means to each single key: the edges to feed their machines. A key's own edges go to it; another
/// key goes to every key a listener carries, to make what is pending not a lone key.
pub fn route(bindings: &[Option<Parsed>; 3], heard: Listened) -> Vec<(SingleKey, Edge)> {
    let mut out: Vec<(SingleKey, Edge)> = Vec::new();
    for binding in bindings.iter().flatten() {
        let Parsed::Single { key, .. } = binding else { continue };
        let edge = match heard {
            Listened::Single { key: pressed, down } if pressed == *key => {
                Some(if down { Edge::Down } else { Edge::Up })
            }
            Listened::Other if key.needs_listener() => Some(Edge::Other),
            _ => None,
        };
        if let Some(edge) = edge {
            if !out.iter().any(|(seen, _)| seen == key) {
                out.push((*key, edge));
            }
        }
    }
    out
}

/// What the machine answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Do the action once.
    Fire,
    HoldStart,
    HoldEnd,
}

/// A combination's gesture over time: edges in, an effect out. The key's own repeat (a second `Down` while it is down) is ignored.
#[derive(Debug, Clone)]
pub struct Machine {
    gesture: Gesture,
    down: bool,
}

impl Machine {
    pub fn new(gesture: Gesture) -> Machine {
        Machine { gesture, down: false }
    }

    pub fn gesture(&self) -> Gesture {
        self.gesture
    }

    pub fn is_down(&self) -> bool {
        self.down
    }

    /// Forgets a press that is on, when the app let go of it (the safety net): a release that comes late is nothing.
    pub fn cancel(&mut self) {
        self.down = false;
    }

    pub fn on(&mut self, edge: Edge) -> Option<Effect> {
        match edge {
            Edge::Down => {
                if self.down {
                    return None;
                }
                self.down = true;
                match self.gesture {
                    Gesture::Press => Some(Effect::Fire),
                    Gesture::Hold => Some(Effect::HoldStart),
                }
            }
            Edge::Other => None,
            Edge::Up => {
                if !std::mem::take(&mut self.down) {
                    return None;
                }
                (self.gesture == Gesture::Hold).then_some(Effect::HoldEnd)
            }
        }
    }
}

/// The ways one physical single key is used: which action each of hold, tap and double does (at most one per way).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Carried {
    pub hold: Option<Action>,
    pub tap: Option<Action>,
    pub double: Option<Action>,
}

impl Carried {
    pub fn ways(&self) -> Ways {
        Ways { hold: self.hold.is_some(), tap: self.tap.is_some(), double: self.double.is_some() }
    }

    pub fn action(&self, trigger: Trigger) -> Option<Action> {
        match trigger {
            Trigger::Hold => self.hold,
            Trigger::Tap => self.tap,
            Trigger::Double => self.double,
        }
    }
}

/// Which ways a key carries: what its machine tells apart by time (only when it is more than one).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ways {
    pub hold: bool,
    pub tap: bool,
    pub double: bool,
}

/// The single keys in use and what each carries, in the order they first appear. `skip` is the actions left out (those
/// refused, or not on): an action left out carries nothing.
pub fn single_keys(parsed: &[Option<Parsed>; 3], skip: &[bool; 3]) -> Vec<(SingleKey, Carried)> {
    let mut keys: Vec<(SingleKey, Carried)> = Vec::new();
    for action in Action::ALL {
        let Some(Parsed::Single { key, trigger }) = &parsed[action.index()] else { continue };
        if skip[action.index()] {
            continue;
        }
        let at = keys.iter().position(|(seen, _)| seen == key).unwrap_or_else(|| {
            keys.push((*key, Carried::default()));
            keys.len() - 1
        });
        let carried = &mut keys[at].1;
        let slot = match trigger {
            Trigger::Hold => &mut carried.hold,
            Trigger::Tap => &mut carried.tap,
            Trigger::Double => &mut carried.double,
        };
        slot.get_or_insert(action);
    }
    keys
}

/// What a single key's machine does, by the way it was pressed.
pub type Pulse = (Trigger, Effect);

/// One physical single key over time: edges and the clock in, what to do out. Time is passed in, so every path is a plain test.
///
/// With more than one way on the key, the ways are told apart by time and only then (`docs/BRIDGE.md` → "One key, several ways"):
/// **hold** starts after [`HOLD_AFTER`] down; a **tap** is a press shorter than that (than [`TAP_WINDOW`] when the key has no hold)
/// with no second press within [`TAP_WINDOW`] after it, and acts when that time is up if the key also has a double; a **double** is
/// a second press within [`TAP_WINDOW`] of the first one's release, acting when it comes up. A key with one way acts as it always did:
/// hold at once, a tap when it comes up, a double when its second tap does. Another key pressed while it is down makes it a
/// modifier of that key and no way acts.
#[derive(Debug, Clone)]
pub struct KeyMachine {
    ways: Ways,
    down_at: Option<Instant>,
    tainted: bool,
    holding: bool,
    /// A first tap that ended at this time, waiting to be a tap or the first of a double.
    pending: Option<Instant>,
    /// The press now down is the second of a pair.
    second: bool,
}

impl KeyMachine {
    pub fn new(ways: Ways) -> KeyMachine {
        KeyMachine { ways, down_at: None, tainted: false, holding: false, pending: None, second: false }
    }

    pub fn ways(&self) -> Ways {
        self.ways
    }

    pub fn is_down(&self) -> bool {
        self.down_at.is_some()
    }

    /// Forgets a press that is on, when the app let go of it (the safety net): a release that comes late is nothing.
    pub fn cancel(&mut self) {
        *self = KeyMachine::new(self.ways);
    }

    /// Whether this key waits to tell a hold from a tap: more than one way on it.
    fn timed(&self) -> bool {
        let Ways { hold, tap, double } = self.ways;
        hold && (tap || double)
    }

    /// The longest a press may be and still be a tap.
    fn tap_limit(&self) -> Duration {
        if self.ways.hold {
            HOLD_AFTER
        } else {
            TAP_WINDOW
        }
    }

    /// When the clock next decides something (`poll` then answers it), if it does.
    pub fn deadline(&self) -> Option<Instant> {
        let hold = self.down_at.filter(|_| self.timed() && !self.holding && !self.tainted).map(|at| at + HOLD_AFTER);
        let tap = self.pending.filter(|_| self.ways.tap).map(|ended| ended + TAP_WINDOW);
        [hold, tap].into_iter().flatten().min()
    }

    /// What the clock alone decides at `now`: a hold that has lasted, a tap no second press followed.
    pub fn poll(&mut self, now: Instant) -> Vec<Pulse> {
        let mut out = Vec::new();
        if let Some(down_at) = self.down_at {
            if self.timed() && !self.holding && !self.tainted && now.saturating_duration_since(down_at) >= HOLD_AFTER {
                self.holding = true;
                self.second = false;
                out.push((Trigger::Hold, Effect::HoldStart));
            }
        }
        if let Some(ended) = self.pending {
            if now.saturating_duration_since(ended) >= TAP_WINDOW {
                self.pending = None;
                if self.ways.tap {
                    out.push((Trigger::Tap, Effect::Fire));
                }
            }
        }
        out
    }

    pub fn on(&mut self, edge: Edge, now: Instant) -> Vec<Pulse> {
        let mut out = self.poll(now);
        match edge {
            Edge::Down => {
                if self.down_at.is_some() {
                    return out;
                }
                self.down_at = Some(now);
                self.tainted = false;
                self.holding = false;
                self.second =
                    self.pending.take().is_some_and(|ended| now.saturating_duration_since(ended) < TAP_WINDOW);
                if self.ways.hold && !self.timed() {
                    self.holding = true;
                    out.push((Trigger::Hold, Effect::HoldStart));
                }
            }
            Edge::Other => {
                if self.down_at.is_some() {
                    self.tainted = true;
                }
                self.pending = None;
                self.second = false;
            }
            Edge::Up => {
                let Some(down_at) = self.down_at.take() else { return out };
                let second = std::mem::take(&mut self.second);
                if self.holding {
                    self.holding = false;
                    out.push((Trigger::Hold, Effect::HoldEnd));
                } else if !self.tainted && now.saturating_duration_since(down_at) < self.tap_limit() {
                    if second && self.ways.double {
                        out.push((Trigger::Double, Effect::Fire));
                    } else if self.ways.double {
                        self.pending = Some(now);
                    } else if self.ways.tap {
                        out.push((Trigger::Tap, Effect::Fire));
                    }
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The stuck hold
// ---------------------------------------------------------------------------------------------------------------------

/// Why a hold was let go by the app, not by the person's hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// [`STUCK_HOLD`] went by with no release.
    TooLong,
    /// The machine slept: a release during it was never seen.
    Slept,
    /// The screen was locked: the keys go to the lock screen, so a release would never come.
    Locked,
    /// The call ended.
    CallEnded,
    /// The shortcut was changed or cleared while it was held.
    Rebound,
}

/// One hold being watched: the shell looks at it every [`WATCH_TICK`] while it lasts.
#[derive(Debug, Clone)]
pub struct Watch {
    started: Instant,
    last_look: SystemTime,
}

impl Watch {
    pub fn new(now: Instant, wall: SystemTime) -> Watch {
        Watch { started: now, last_look: wall }
    }

    /// The reason to let this hold go at this look, if there is one. `wall` is the clock that keeps running while the machine
    /// sleeps (a monotonic one may stop); `locked` is whether the screen is locked now.
    pub fn look(&mut self, now: Instant, wall: SystemTime, locked: bool) -> Option<Release> {
        let gap = wall.duration_since(self.last_look).unwrap_or_default();
        self.last_look = wall;
        if gap > SLEEP_GAP {
            return Some(Release::Slept);
        }
        if locked {
            return Some(Release::Locked);
        }
        (now.saturating_duration_since(self.started) >= STUCK_HOLD).then_some(Release::TooLong)
    }
}

/// Whether a hold is on, and whose watch it is: a watch that is not the current one has nothing to do.
#[derive(Debug, Default)]
pub struct Hold {
    generation: u64,
    on: bool,
}

impl Hold {
    /// A hold begins: the generation its watch carries. A second begin while one is on changes nothing.
    pub fn begin(&mut self) -> Option<u64> {
        if self.on {
            return None;
        }
        self.on = true;
        self.generation += 1;
        Some(self.generation)
    }

    /// The hold ends, by a release or by the safety net. Whether one was on.
    pub fn end(&mut self) -> bool {
        std::mem::replace(&mut self.on, false)
    }

    /// Whether the hold of `generation` is still the one on.
    pub fn is_current(&self, generation: u64) -> bool {
        self.on && self.generation == generation
    }

    pub fn is_on(&self) -> bool {
        self.on
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Registrations follow the settings
// ---------------------------------------------------------------------------------------------------------------------

/// What the system's hot keys are asked, one at a time. A registration is only ever the app's own.
pub trait Registrar {
    /// Whether the system took the hot key.
    fn register(&mut self, hot_key: &str) -> bool;
    /// Whether the app held it and has let it go.
    fn unregister(&mut self, hot_key: &str) -> bool;
}

/// The systems the app registers global hot keys on, for the keys each can and cannot carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Macos,
    Windows,
    Linux,
}

/// What a system can do with the key of a combination, which `global-hotkey` (the backend) decides by its own tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyReach {
    /// The key registers as the key it is.
    Physical,
    /// X11 registers it by the character the layout types on it, not by its position: another key, or none, under a
    /// layout that is not US. Kept, with a warning.
    ByLayout,
    /// The backend cannot register this key, or registers another key in its place (Windows maps `NumpadEqual` to E and
    /// `NumpadEnter` to Enter). Refused: never another key in its place.
    Never,
}

/// How `platform` registers `key` (a key's name in an accelerator, as [`settings::normalize_shortcut`] writes it).
///
/// From `global-hotkey` 0.8's tables: no system maps `IntlBackslash` (the ISO `<>` key; macOS `kVK_ISO_Section`, Windows
/// `VK_OEM_102`, an X11 keysym); Windows maps `NumpadEqual` to `VK_E` and `NumpadEnter` to `VK_RETURN`; X11 has no keysym
/// for either keypad key and registers the punctuation keys by keysym.
pub fn key_reach(platform: Platform, key: &str) -> KeyReach {
    match (platform, key) {
        (_, "IntlBackslash") => KeyReach::Never,
        (Platform::Windows | Platform::Linux, "NumpadEqual" | "NumpadEnter") => KeyReach::Never,
        (
            Platform::Linux,
            "Minus" | "Equal" | "Comma" | "Period" | "Slash" | "Backslash" | "Semicolon" | "Quote" | "BracketLeft"
            | "BracketRight" | "Backquote",
        ) => KeyReach::ByLayout,
        _ => KeyReach::Physical,
    }
}

/// [`key_reach`] of the key of a hot key (`Ctrl+Alt+BracketRight`, `F9`).
pub fn hot_key_reach(platform: Platform, hot_key: &str) -> KeyReach {
    key_reach(platform, hot_key.rsplit('+').next().unwrap_or(hot_key))
}

/// The hot keys the app holds now, one slot per action (in [`Action::ALL`] order).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Held([Option<String>; 3]);

impl Held {
    pub fn of(&self, action: Action) -> Option<&str> {
        self.0[action.index()].as_deref()
    }
}

/// How the system took what one action asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Taken {
    pub state: Registration,
    pub alternatives: Vec<String>,
    /// Refused because this other action of ours holds the keys.
    pub holder: Option<Action>,
}

/// What the app offers an action when its keys are taken, one for each action in [`Action::ALL`] order, if the system takes it
/// and nothing else of ours holds it: common keys, F9 to F12 with Ctrl and Shift (no F13 to F19, which most keyboards do not
/// have). Checked against the systems' default lists (docs/SHORTCUTS.md → "What the app offers instead"): not F10, which opens a
/// menu bar on Windows and Linux, and not with Option and Shift on a Mac, where those keys with the volume and brightness keys
/// make fine adjustments; nor Control with F2 to F8, keyboard navigation.
pub const ALTERNATIVES: [&str; 3] = ["Ctrl+Shift+F9", "Ctrl+Shift+F11", "Ctrl+Shift+F12"];

/// What the app offers `action` in place of `asked`: not what anything is asking for, and only if the system takes it.
pub fn alternatives_for<R: Registrar>(
    registrar: &mut R,
    wanted: &[Option<String>; 3],
    action: Action,
    asked: &str,
) -> Vec<String> {
    [ALTERNATIVES[action.index()]]
        .into_iter()
        .filter(|a| !settings::same_shortcut(a, asked))
        .filter(|a| !wanted.iter().flatten().any(|w| settings::same_shortcut(w, a)))
        .filter(|a| registrar.register(a) && registrar.unregister(a))
        .map(str::to_owned)
        .collect()
}

/// Which actions are refused for using a single key in the same way as an earlier action (`RightShift:double` twice, `F9:tap`
/// twice): the earlier one holds it, and is named. Different ways of one key (`RightShift:hold`, `RightShift:double`) coexist: a
/// [`KeyMachine`] tells them apart. The first of [`Action::ALL`] holds a key.
pub fn duplicates(parsed: &[Option<Parsed>; 3]) -> [Option<Action>; 3] {
    std::array::from_fn(|i| {
        let Some(binding @ Parsed::Single { .. }) = &parsed[i] else { return None };
        Action::ALL[..i].iter().copied().find(|earlier| parsed[earlier.index()].as_ref() == Some(binding))
    })
}

/// Which actions use an F-key in another way than an earlier action does: the system has one hot key for the F-key, the
/// earlier action's, and the later one rides on it (its state is that action's, and a [`KeyMachine`] tells the ways apart).
/// `duplicates` are left out.
pub fn rides(parsed: &[Option<Parsed>; 3], duplicate: &[Option<Action>; 3]) -> [Option<Action>; 3] {
    std::array::from_fn(|i| {
        let Some(Parsed::Single { key: key @ SingleKey::Function(_), .. }) = &parsed[i] else { return None };
        if duplicate[i].is_some() {
            return None;
        }
        Action::ALL[..i].iter().copied().find(|earlier| {
            duplicate[earlier.index()].is_none()
                && matches!(&parsed[earlier.index()], Some(Parsed::Single { key: other, .. }) if other == key)
        })
    })
}

/// Brings the system's hot keys to what `wanted` says, one entry per action: only what changed is touched (a diff, never
/// everything off and on again), and what is asked is told how the system took it. `None` is an action with no hot key
/// (none set, or a key a listener carries). `backend` is whether this session has hot keys at all.
///
/// Two actions on the same keys: the first of [`Action::ALL`] holds them, the other is `Refused` and offered others.
/// A key the system says is taken is first let go of by the app, in case it is the app's own, held on by a registration
/// the app lost track of, before it is believed to be another app's: so a shortcut never stays refused for good because of
/// a stale registration (the Windows report of "another app already uses it", docs/SHORTCUTS.md).
pub fn reconcile<R: Registrar>(
    registrar: &mut R,
    held: &mut Held,
    wanted: &[Option<String>; 3],
    backend: bool,
) -> [Option<Taken>; 3] {
    // Let go of what is no longer wanted, or is wanted by another action that comes first.
    let mut owner: [Option<usize>; 3] = [None; 3];
    let mut holder: [Option<Action>; 3] = [None; 3];
    for (i, want) in wanted.iter().enumerate() {
        if let Some(keys) = want {
            match wanted[..i].iter().position(|earlier| earlier.as_ref() == Some(keys)) {
                None => owner[i] = Some(i),
                Some(first) => holder[i] = Some(Action::ALL[first]),
            }
        }
    }
    for (i, slot) in held.0.iter_mut().enumerate() {
        let keep = owner[i].is_some() && slot.as_deref() == wanted[i].as_deref();
        if !keep {
            if let Some(keys) = slot.take() {
                if !registrar.unregister(&keys) {
                    log::debug!("shortcut {keys} was not held when it was let go");
                }
            }
        }
    }
    let mut taken: [Option<Taken>; 3] = [None, None, None];
    for i in 0..3 {
        let Some(keys) = &wanted[i] else { continue };
        if owner[i].is_none() {
            taken[i] = Some(Taken { state: Registration::Refused, alternatives: vec![], holder: holder[i] });
        } else if !backend {
            taken[i] = Some(Taken { state: Registration::Unavailable, alternatives: vec![], holder: None });
        } else if held.0[i].is_some() {
            taken[i] = Some(Taken { state: Registration::On, alternatives: vec![], holder: None });
        } else if registrar.register(keys) || {
            let _ = registrar.unregister(keys);
            registrar.register(keys)
        } {
            held.0[i] = Some(keys.clone());
            taken[i] = Some(Taken { state: Registration::On, alternatives: vec![], holder: None });
        } else {
            log::debug!("shortcut {keys} refused");
            taken[i] = Some(Taken { state: Registration::Refused, alternatives: vec![], holder: None });
        }
    }
    // The alternatives, once everything wanted is placed: not what anything is asking for, and only what the system takes.
    for i in 0..3 {
        if !taken[i].as_ref().is_some_and(|t| t.state == Registration::Refused) {
            continue;
        }
        let asked = wanted[i].as_deref().unwrap_or_default();
        let alternatives = alternatives_for(registrar, wanted, Action::ALL[i], asked);
        taken[i] = Some(Taken { state: Registration::Refused, alternatives, holder: holder[i] });
    }
    taken
}

// ---------------------------------------------------------------------------------------------------------------------
// What the page is told
// ---------------------------------------------------------------------------------------------------------------------

/// What a single key on its own needs from the system (macOS: Input Monitoring).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionKind {
    InputMonitoring,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionState {
    Granted,
    /// Never asked here: the person has not chosen Allow.
    NotAsked,
    /// Asked, and the system did not grant it.
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Permission {
    pub kind: PermissionKind,
    pub state: PermissionState,
}

/// What a headset's press does: the single press (play/pause) or the double press (the system's next track).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum HeadsetPress {
    #[default]
    Mute,
    OneTurn,
    Skip,
    None,
}

impl HeadsetPress {
    pub fn parse(name: &str) -> Option<HeadsetPress> {
        match name {
            "mute" => Some(HeadsetPress::Mute),
            "oneTurn" => Some(HeadsetPress::OneTurn),
            "skip" => Some(HeadsetPress::Skip),
            "none" => Some(HeadsetPress::None),
            _ => None,
        }
    }
}

/// What a headset's buttons do here: `host.shortcuts`' `HeadsetStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Headset {
    pub press: HeadsetPress,
    /// A double press arrives as the system's next-track command, which the app answers while it holds the media session.
    /// Always there where the app owns the headset; `Option` is the page's `null`: a path that cannot send one.
    pub double_press: Option<HeadsetPress>,
    /// The headset only acts in a call: the app leaves the buttons to other apps outside one.
    pub outside_call: &'static str,
}

impl Headset {
    pub fn new(press: HeadsetPress, double_press: HeadsetPress) -> Headset {
        Headset { press, double_press: Some(double_press), outside_call: "leave" }
    }
}

/// `host.shortcuts.get()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub shortcuts: Shortcuts,
    /// What the system must still allow for a single key set now; `null` when no single key needs anything.
    pub permission: Option<Permission>,
    /// `null`: no headset path on this host.
    pub headset: Option<Headset>,
}

/// The three, keyed by action.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shortcuts {
    pub mute: GlobalShortcut,
    pub hold_to_talk: GlobalShortcut,
    pub one_turn: GlobalShortcut,
}

impl Shortcuts {
    pub fn from_array(all: [GlobalShortcut; 3]) -> Shortcuts {
        let [mute, hold_to_talk, one_turn] = all;
        Shortcuts { mute, hold_to_talk, one_turn }
    }

    pub fn get(&self, action: Action) -> &GlobalShortcut {
        match action {
            Action::Mute => &self.mute,
            Action::HoldToTalk => &self.hold_to_talk,
            Action::OneTurn => &self.one_turn,
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The defaults and the one migration
// ---------------------------------------------------------------------------------------------------------------------

/// Whether the person is still on the old default of the mute shortcut and this system now has another: only then is it moved,
/// once. A shortcut chosen since stays, whatever it is, and so does one chosen after the move.
pub fn migrated_mute(stored: &str, already: bool, default: &str) -> Option<String> {
    (!already
        && settings::same_shortcut(stored, LEGACY_MUTE_SHORTCUT)
        && !settings::same_shortcut(default, LEGACY_MUTE_SHORTCUT))
    .then(|| default.to_owned())
}

// ---------------------------------------------------------------------------------------------------------------------
// Capturing the next key (macOS: the app hears keys, Fn included, in its own window with no permission)
// ---------------------------------------------------------------------------------------------------------------------

/// The modifiers held while a key goes down.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub command: bool,
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Mods {
    /// The modifiers held in the flags a macOS event carries.
    pub fn from_macos_flags(flags: u64) -> Mods {
        Mods {
            command: flags & 0x0010_0000 != 0,
            control: flags & 0x0004_0000 != 0,
            alt: flags & 0x0008_0000 != 0,
            shift: flags & 0x0002_0000 != 0,
        }
    }
}

/// What the recorder hears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Heard {
    /// A single-key modifier (or Fn) goes down or up.
    Modifier { key: SingleKey, down: bool },
    /// A left-hand modifier goes down or up: one that cannot be a key on its own.
    LeftModifier { down: bool },
    /// Another key goes down, with the modifiers held: its macOS virtual key code, and the character it types with none.
    Key { code: u16, character: Option<String>, mods: Mods },
}

/// Why a key pressed while recording was not taken (`docs/BRIDGE.md` → `record`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Hint {
    /// A left-hand modifier on its own, where only the right-hand ones can be a key (macOS).
    LeftModifier,
    /// A modifier or Fn on its own, where the system gives the app no way to hear one (Windows, Linux).
    LoneModifier,
}

/// The recorder's answer so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recorded {
    /// Keep listening.
    Pending,
    /// A key was ignored and the recording goes on: the page says why.
    Hint(Hint),
    /// Done: the accelerator, or `None` for Escape.
    Done(Option<String>),
}

/// Captures the next shortcut for one action: a combination (a modifier, or an F-key), or a modifier / Fn on its own in the
/// way that action allows (hold to talk: held; mute: tapped; one turn: tapped, or double tapped).
#[derive(Debug, Clone)]
pub struct Recorder {
    action: Action,
    down: Option<SingleKey>,
    tainted: bool,
    /// A tap that may become a double one: the key and when it ended.
    tapped: Option<(SingleKey, Instant)>,
    down_at: Option<Instant>,
    /// Whether this system lets a modifier or Fn be a key on its own (macOS: the right-hand ones and Fn). Where not, one
    /// pressed alone is ignored and the recording goes on for a real key.
    lone_keys: bool,
    /// A left-hand modifier is down, and no other key has been pressed meanwhile.
    left: Option<bool>,
}

impl Recorder {
    /// A recorder on a system that takes the right-hand modifiers and Fn on their own (macOS).
    pub fn new(action: Action) -> Recorder {
        Recorder::on(action, true)
    }

    /// A recorder for `action`; `lone_keys`: whether this system can have a modifier or Fn on its own as a key.
    pub fn on(action: Action, lone_keys: bool) -> Recorder {
        Recorder { action, down: None, tainted: false, tapped: None, down_at: None, lone_keys, left: None }
    }

    fn ignored(&self) -> Recorded {
        Recorded::Hint(if self.lone_keys { Hint::LeftModifier } else { Hint::LoneModifier })
    }

    fn done_single(&self, key: SingleKey, trigger: Trigger) -> Recorded {
        Recorded::Done(Some(Parsed::Single { key, trigger }.accelerator()))
    }

    pub fn hear(&mut self, heard: Heard, now: Instant) -> Recorded {
        match heard {
            Heard::Modifier { key, down: true } => {
                self.down = Some(key);
                self.down_at = Some(now);
                self.tainted = false;
                Recorded::Pending
            }
            Heard::Modifier { key, down: false } => {
                let (Some(held), Some(at)) = (self.down.take(), self.down_at.take()) else { return Recorded::Pending };
                if held != key || self.tainted {
                    return Recorded::Pending;
                }
                if !self.lone_keys {
                    return Recorded::Hint(Hint::LoneModifier);
                }
                match self.action {
                    Action::HoldToTalk => self.done_single(key, Trigger::Hold),
                    Action::Mute => self.done_single(key, Trigger::Tap),
                    Action::OneTurn => {
                        if now.saturating_duration_since(at) >= TAP_WINDOW {
                            return Recorded::Pending;
                        }
                        match self.tapped.take() {
                            Some((first, ended))
                                if first == key && now.saturating_duration_since(ended) < TAP_WINDOW =>
                            {
                                self.done_single(key, Trigger::Double)
                            }
                            _ => {
                                self.tapped = Some((key, now));
                                Recorded::Pending
                            }
                        }
                    }
                }
            }
            Heard::LeftModifier { down: true } => {
                self.left = Some(true);
                Recorded::Pending
            }
            Heard::LeftModifier { down: false } => match self.left.take() {
                Some(true) => self.ignored(),
                _ => Recorded::Pending,
            },
            Heard::Key { code, character, mods } => {
                self.tainted = true;
                self.left = self.left.map(|_| false);
                self.tapped = None;
                let none = !(mods.command || mods.control || mods.alt || mods.shift);
                if code == KEY_ESCAPE && none {
                    return Recorded::Done(None);
                }
                let Some(key) = key_name(code, character.as_deref()) else { return Recorded::Pending };
                if none && function_key(&key).is_none() {
                    return Recorded::Pending; // a key on its own would be taken from every app
                }
                let mut text = String::new();
                for (held, name) in
                    [(mods.command, "Cmd"), (mods.control, "Ctrl"), (mods.alt, "Alt"), (mods.shift, "Shift")]
                {
                    if held {
                        text.push_str(name);
                        text.push('+');
                    }
                }
                text.push_str(&key);
                match settings::normalize_shortcut(&text) {
                    Some(keys) if !keys.is_empty() => Recorded::Done(Some(keys)),
                    _ => Recorded::Pending,
                }
            }
        }
    }

    /// A tap waiting to see whether a second one follows becomes the tap it was once [`TAP_WINDOW`] has gone by.
    pub fn settle(&mut self, now: Instant) -> Recorded {
        match self.tapped {
            Some((key, ended)) if now.saturating_duration_since(ended) >= TAP_WINDOW => {
                self.tapped = None;
                self.done_single(key, Trigger::Tap)
            }
            _ => Recorded::Pending,
        }
    }
}

const KEY_ESCAPE: u16 = 53;

/// The key's name in an accelerator, from a macOS virtual key code: the key itself, never the character it types. A
/// character does not say which key it came from (keypad `-` and the main row's are both `-`, `+` is the accelerator's own
/// separator) and moves with the layout; the code does not. Only a key this table has no code for (an ISO section key, say)
/// is named by its character, when that is one the accelerator can carry.
fn key_name(code: u16, character: Option<&str>) -> Option<String> {
    let named = match code {
        0 => "A",
        11 => "B",
        8 => "C",
        2 => "D",
        14 => "E",
        3 => "F",
        5 => "G",
        4 => "H",
        34 => "I",
        38 => "J",
        40 => "K",
        37 => "L",
        46 => "M",
        45 => "N",
        31 => "O",
        35 => "P",
        12 => "Q",
        15 => "R",
        1 => "S",
        17 => "T",
        32 => "U",
        9 => "V",
        13 => "W",
        7 => "X",
        16 => "Y",
        6 => "Z",
        29 => "0",
        18 => "1",
        19 => "2",
        20 => "3",
        21 => "4",
        23 => "5",
        22 => "6",
        26 => "7",
        28 => "8",
        25 => "9",
        27 => "Minus",
        24 => "Equal",
        43 => "Comma",
        47 => "Period",
        44 => "Slash",
        42 => "Backslash",
        41 => "Semicolon",
        39 => "Quote",
        33 => "BracketLeft",
        30 => "BracketRight",
        50 => "Backquote",
        10 => "IntlBackslash",
        69 => "NumpadAdd",
        78 => "NumpadSubtract",
        67 => "NumpadMultiply",
        75 => "NumpadDivide",
        65 => "NumpadDecimal",
        81 => "NumpadEqual",
        76 => "NumpadEnter",
        82 => "Numpad0",
        83 => "Numpad1",
        84 => "Numpad2",
        85 => "Numpad3",
        86 => "Numpad4",
        87 => "Numpad5",
        88 => "Numpad6",
        89 => "Numpad7",
        91 => "Numpad8",
        92 => "Numpad9",
        122 => "F1",
        120 => "F2",
        99 => "F3",
        118 => "F4",
        96 => "F5",
        97 => "F6",
        98 => "F7",
        100 => "F8",
        101 => "F9",
        109 => "F10",
        103 => "F11",
        111 => "F12",
        105 => "F13",
        107 => "F14",
        113 => "F15",
        106 => "F16",
        64 => "F17",
        79 => "F18",
        80 => "F19",
        90 => "F20",
        49 => "Space",
        36 => "Enter",
        48 => "Tab",
        51 => "Backspace",
        117 => "Delete",
        126 => "ArrowUp",
        125 => "ArrowDown",
        123 => "ArrowLeft",
        124 => "ArrowRight",
        115 => "Home",
        119 => "End",
        116 => "PageUp",
        121 => "PageDown",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.to_owned());
    }
    let mut chars = character?.chars();
    let first = chars.next()?;
    (chars.next().is_none() && !first.is_control() && !matches!(first, '+' | ':'))
        .then(|| first.to_uppercase().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> Instant {
        static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *ORIGIN.get_or_init(Instant::now) + Duration::from_millis(ms)
    }

    #[test]
    fn accelerators_are_one_form_for_the_web_and_the_app() {
        for (text, kept) in [
            ("Ctrl+Alt+Shift+M", "Ctrl+Alt+Shift+M"),
            ("control+option+shift+m", "Ctrl+Alt+Shift+M"),
            ("Cmd+Shift+M", "Cmd+Shift+M"),
            ("F9", "F9"),
            ("Fn:hold", "Fn:hold"),
            ("RightControl:double", "RightControl:double"),
            ("RightAlt:TAP", "RightAlt:tap"),
            ("RightCommand:tap", "RightCommand:tap"),
            ("F13:hold", "F13:hold"),
        ] {
            assert_eq!(parse(text).unwrap().unwrap().accelerator(), kept, "{text}");
        }
        assert_eq!(parse("  "), Some(None), "empty: none");
        for wrong in
            ["Fn", "M", "F25", "F0", "F013", "Ctrl+Fn:hold", "Fn:long", "LeftShift:tap", "Ctrl+M:tap", "Hyper+M"]
        {
            assert_eq!(parse(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn a_hot_key_is_for_a_combination_or_an_f_key_and_a_listener_for_the_rest() {
        let hot = |text: &str| parse(text).unwrap().unwrap().hot_key();
        assert_eq!(hot("Ctrl+Alt+M").as_deref(), Some("Ctrl+Alt+M"));
        assert_eq!(hot("F9").as_deref(), Some("F9"));
        assert_eq!(hot("F13:tap").as_deref(), Some("F13"));
        assert_eq!(hot("Fn:hold"), None);
        assert_eq!(hot("RightControl:double"), None);
        let listened = |text: &str| parse(text).unwrap().unwrap().listened();
        assert_eq!(listened("Fn:hold"), Some(SingleKey::Fn));
        assert_eq!(listened("RightShift:tap"), Some(SingleKey::RightShift));
        assert_eq!(listened("F13:tap"), None);
        assert_eq!(listened("Ctrl+M"), None);
    }

    #[test]
    fn an_action_takes_only_the_keys_it_can_be_done_with() {
        let p = |t: &str| parse(t).unwrap().unwrap();
        assert!(suits(Action::HoldToTalk, &p("Fn:hold")) && !suits(Action::HoldToTalk, &p("Fn:tap")));
        assert!(suits(Action::Mute, &p("RightAlt:tap")) && suits(Action::Mute, &p("RightAlt:double")));
        assert!(!suits(Action::Mute, &p("RightAlt:hold")));
        assert!(suits(Action::OneTurn, &p("RightAlt:double")));
        for action in Action::ALL {
            assert!(suits(action, &p("Ctrl+Alt+M")));
        }
    }

    #[test]
    fn labels_read_as_the_system_writes_keys() {
        assert_eq!(label("Ctrl+Alt+M", true, "en"), "⌃⌥M");
        assert_eq!(label("Ctrl+Alt+Shift+M", false, "en"), "Ctrl+Alt+Shift+M");
        assert_eq!(label("Fn:hold", true, "en"), "Fn");
        assert_eq!(label("RightControl:double", true, "en"), "Right ⌃ ×2");
        assert_eq!(label("RightAlt:tap", false, "en"), "Right Alt");
        assert_eq!(label("RightAlt:tap", false, "es"), "Der. Alt");
        assert_eq!(label("F13:tap", false, "en"), "F13");
    }

    #[test]
    fn a_combination_acts_when_pressed_and_hold_to_talk_while_held() {
        let combo = parse("Ctrl+Alt+M").unwrap().unwrap();
        assert_eq!(gesture(Action::Mute, &combo), Some(Gesture::Press));
        assert_eq!(gesture(Action::OneTurn, &combo), Some(Gesture::Press));
        assert_eq!(gesture(Action::HoldToTalk, &combo), Some(Gesture::Hold));
        assert_eq!(
            gesture(Action::Mute, &parse("RightShift:tap").unwrap().unwrap()),
            None,
            "a single key has a KeyMachine"
        );
        let mut press = Machine::new(Gesture::Press);
        assert_eq!(press.on(Edge::Down), Some(Effect::Fire));
        assert_eq!(press.on(Edge::Down), None, "its own repeat");
        assert_eq!(press.on(Edge::Up), None);
        assert_eq!(press.on(Edge::Down), Some(Effect::Fire));
        let mut hold = Machine::new(Gesture::Hold);
        assert_eq!(hold.on(Edge::Down), Some(Effect::HoldStart));
        hold.cancel();
        assert_eq!(hold.on(Edge::Up), None, "a release after the app let go is nothing");
        assert_eq!(hold.on(Edge::Down), Some(Effect::HoldStart));
        assert_eq!(hold.on(Edge::Down), None);
        assert_eq!(hold.on(Edge::Other), None, "other keys do not end a hold");
        assert_eq!(hold.on(Edge::Up), Some(Effect::HoldEnd));
        assert_eq!(hold.on(Edge::Up), None, "a release without a press is nothing");
    }

    // ----- one physical key, several ways (R10) -----

    use Edge::{Down, Other, Up};
    use Effect::{Fire, HoldEnd, HoldStart};
    use Trigger::{Double, Tap};
    const HOLD: Trigger = Trigger::Hold;

    fn ways(hold: bool, tap: bool, double: bool) -> KeyMachine {
        KeyMachine::new(Ways { hold, tap, double })
    }

    /// What `key` does for these edges `(edge, ms)`, polling the clock at each of `polls` too: everything it said, in order.
    fn run(key: &mut KeyMachine, steps: &[(Edge, u64)], polls: &[u64]) -> Vec<(u64, Pulse)> {
        let mut at_ms: Vec<(u64, Option<Edge>)> =
            steps.iter().map(|(edge, ms)| (*ms, Some(*edge))).chain(polls.iter().map(|ms| (*ms, None))).collect();
        at_ms.sort_by_key(|(ms, edge)| (*ms, edge.is_some()));
        let mut said = Vec::new();
        for (ms, edge) in at_ms {
            let pulses = match edge {
                Some(edge) => key.on(edge, at(ms)),
                None => key.poll(at(ms)),
            };
            said.extend(pulses.into_iter().map(|pulse| (ms, pulse)));
        }
        said
    }

    #[test]
    fn a_hold_on_its_own_opens_at_once_and_ends_when_let_go() {
        let mut key = ways(true, false, false);
        assert_eq!(run(&mut key, &[(Down, 0), (Up, 3000)], &[]), [(0, (HOLD, HoldStart)), (3000, (HOLD, HoldEnd))]);
        assert_eq!(key.deadline(), None, "no clock to wait for");
        // A tap of it is a short hold, nothing more.
        assert_eq!(
            run(&mut key, &[(Down, 5000), (Up, 5050)], &[]),
            [(5000, (HOLD, HoldStart)), (5050, (HOLD, HoldEnd))]
        );
        // Another key typed meanwhile does not end it (as it never did): the key is held for typing capitals.
        assert_eq!(
            run(&mut key, &[(Down, 6000), (Other, 6100), (Up, 6200)], &[]),
            [(6000, (HOLD, HoldStart)), (6200, (HOLD, HoldEnd))]
        );
    }

    #[test]
    fn a_tap_on_its_own_acts_when_it_comes_up() {
        let mut key = ways(false, true, false);
        assert_eq!(run(&mut key, &[(Down, 0), (Up, 399)], &[]), [(399, (Tap, Fire))]);
        assert_eq!(run(&mut key, &[(Down, 1000), (Up, 1400)], &[]), [], "400 ms is a hold, not a tap");
        // Cmd+C: the key was a modifier of another.
        assert_eq!(run(&mut key, &[(Down, 2000), (Other, 2050), (Up, 2100)], &[]), []);
        assert_eq!(
            run(&mut key, &[(Down, 3000), (Up, 3100)], &[]),
            [(3100, (Tap, Fire))],
            "and the next lone tap is a tap again"
        );
        assert_eq!(key.deadline(), None);
    }

    #[test]
    fn a_double_on_its_own_is_two_taps_close_together() {
        let mut key = ways(false, false, true);
        assert_eq!(run(&mut key, &[(Down, 0), (Up, 80)], &[]), [], "the first tap only waits");
        assert_eq!(run(&mut key, &[(Down, 200), (Up, 280)], &[]), [(280, (Double, Fire))]);
        // The pair is spent: a third tap starts over.
        assert_eq!(run(&mut key, &[(Down, 300), (Up, 360)], &[]), []);
        // Too far apart: 400 ms or more from the first one's release.
        assert_eq!(run(&mut key, &[(Down, 760), (Up, 800)], &[]), []);
        assert_eq!(
            run(&mut key, &[(Down, 900), (Up, 960)], &[]),
            [(960, (Double, Fire))],
            "99 ms after the last release"
        );
        assert_eq!(
            run(&mut key, &[(Down, 1000), (Up, 1040), (Down, 1440), (Up, 1480)], &[]),
            [],
            "exactly 400 ms is too far"
        );
        // A key typed between the two taps breaks the pair.
        assert_eq!(run(&mut key, &[(Down, 3000), (Up, 3040), (Other, 3050), (Down, 3100), (Up, 3150)], &[]), []);
        // A second press that goes on is not a tap.
        assert_eq!(run(&mut key, &[(Down, 4000), (Up, 4040), (Down, 4100), (Up, 4600)], &[]), []);
    }

    #[test]
    fn hold_and_double_on_one_key_a_double_does_not_flash_the_microphone() {
        let mut key = ways(true, false, true);
        // A hold alone: nothing at once, it opens after 200 ms with the key still down.
        assert_eq!(key.deadline(), None);
        assert_eq!(key.on(Down, at(0)), []);
        assert_eq!(key.deadline(), Some(at(200)));
        assert_eq!(key.poll(at(199)), []);
        assert_eq!(key.poll(at(200)), [(HOLD, HoldStart)]);
        assert_eq!(key.deadline(), None);
        assert_eq!(key.on(Up, at(2500)), [(HOLD, HoldEnd)]);
        // A double: two quick presses, no HoldStart anywhere in between.
        assert_eq!(
            run(&mut key, &[(Down, 5000), (Up, 5100), (Down, 5300), (Up, 5400)], &[5150, 5350, 5500, 6000]),
            [(5400, (Double, Fire))]
        );
        // One quick press alone is neither (there is no tap here), and nothing opened.
        assert_eq!(run(&mut key, &[(Down, 8000), (Up, 8100)], &[8300, 9000]), []);
        // A second press that is held is a hold, 200 ms into it.
        assert_eq!(
            run(&mut key, &[(Down, 10_000), (Up, 10_100), (Down, 10_300), (Up, 11_000)], &[10_500]),
            [(10_500, (HOLD, HoldStart)), (11_000, (HOLD, HoldEnd))]
        );
        // A press polled late: the hold still starts before its release.
        assert_eq!(
            run(&mut key, &[(Down, 12_000), (Up, 12_900)], &[]),
            [(12_900, (HOLD, HoldStart)), (12_900, (HOLD, HoldEnd))]
        );
        // Typing a capital with the key: a modifier, no way acts.
        assert_eq!(run(&mut key, &[(Down, 14_000), (Other, 14_050), (Up, 14_100)], &[14_300]), []);
        assert_eq!(run(&mut key, &[(Down, 15_000), (Other, 15_050)], &[15_300]), [], "and no hold opens behind it");
    }

    #[test]
    fn tap_and_double_on_one_key_the_tap_waits_to_see_whether_a_second_follows() {
        let mut key = ways(false, true, true);
        // A tap alone acts when the double's time is up, not before.
        assert_eq!(key.on(Down, at(0)), []);
        assert_eq!(key.on(Up, at(80)), []);
        assert_eq!(key.deadline(), Some(at(480)));
        assert_eq!(key.poll(at(479)), []);
        assert_eq!(key.poll(at(480)), [(Tap, Fire)]);
        assert_eq!(key.deadline(), None);
        // A double acts when its second press comes up, and the tap does not.
        assert_eq!(
            run(&mut key, &[(Down, 1000), (Up, 1080), (Down, 1300), (Up, 1380)], &[1500, 2000]),
            [(1380, (Double, Fire))]
        );
        // The second press at 399 ms is still a double, at 400 ms it is the next tap's first.
        assert_eq!(
            run(&mut key, &[(Down, 3000), (Up, 3050), (Down, 3449), (Up, 3500)], &[4000]),
            [(3500, (Double, Fire))]
        );
        assert_eq!(
            run(&mut key, &[(Down, 5000), (Up, 5050), (Down, 5450), (Up, 5500)], &[5450, 6000]),
            [(5450, (Tap, Fire)), (6000, (Tap, Fire))],
            "400 ms after the release: the first is a tap on its own, the second a tap on its own"
        );
        // Without a hold a tap may last up to 400 ms.
        assert_eq!(run(&mut key, &[(Down, 8000), (Up, 8399)], &[9000]), [(8800, (Tap, Fire))].map(|(_, p)| (9000, p)));
        // A press that goes on is nothing; another key makes it a modifier.
        assert_eq!(run(&mut key, &[(Down, 10_000), (Up, 10_400)], &[11_000]), []);
        assert_eq!(run(&mut key, &[(Down, 12_000), (Other, 12_010), (Up, 12_050)], &[13_000]), []);
        // Another key between the two taps: the first stays a tap? No: it was broken by a key typed, so it is nothing.
        assert_eq!(run(&mut key, &[(Down, 14_000), (Up, 14_040), (Other, 14_100)], &[15_000]), []);
    }

    #[test]
    fn hold_and_tap_on_one_key() {
        let mut key = ways(true, true, false);
        assert_eq!(
            run(&mut key, &[(Down, 0), (Up, 150)], &[]),
            [(150, (Tap, Fire))],
            "a tap acts when it comes up: no double to wait for"
        );
        assert_eq!(
            run(&mut key, &[(Down, 1000), (Up, 1300)], &[1200]),
            [(1200, (HOLD, HoldStart)), (1300, (HOLD, HoldEnd))]
        );
        // 199 ms is a tap, 200 ms is a hold.
        assert_eq!(run(&mut key, &[(Down, 2000), (Up, 2199)], &[]), [(2199, (Tap, Fire))]);
        assert_eq!(
            run(&mut key, &[(Down, 3000), (Up, 3200)], &[]),
            [(3200, (HOLD, HoldStart)), (3200, (HOLD, HoldEnd))]
        );
    }

    #[test]
    fn all_three_on_one_key() {
        let mut key = ways(true, true, true);
        // Hold.
        assert_eq!(
            run(&mut key, &[(Down, 0), (Up, 1000)], &[200]),
            [(200, (HOLD, HoldStart)), (1000, (HOLD, HoldEnd))]
        );
        // Tap: when the double's time is up.
        assert_eq!(run(&mut key, &[(Down, 2000), (Up, 2100)], &[2200, 2499, 2500, 2501]), [(2500, (Tap, Fire))]);
        // Double.
        assert_eq!(
            run(&mut key, &[(Down, 4000), (Up, 4100), (Down, 4300), (Up, 4400)], &[4200, 4350, 5000]),
            [(4400, (Double, Fire))]
        );
        // A tap and then a hold: the tap acts when the hold starts is not a thing; the hold wins the second press.
        assert_eq!(
            run(&mut key, &[(Down, 6000), (Up, 6100), (Down, 6300), (Up, 7000)], &[6500]),
            [(6500, (HOLD, HoldStart)), (7000, (HOLD, HoldEnd))]
        );
    }

    #[test]
    fn a_machine_that_is_let_go_forgets_everything() {
        let mut key = ways(true, true, true);
        key.on(Down, at(0));
        key.poll(at(300));
        key.cancel();
        assert_eq!(key.on(Up, at(500)), [], "a release after the app let go is nothing");
        assert_eq!(key.deadline(), None);
        assert!(!key.is_down());
        key.on(Down, at(1000));
        key.on(Up, at(1050));
        assert!(key.deadline().is_some());
        key.cancel();
        assert_eq!(key.deadline(), None, "a pending tap is forgotten too");
    }

    #[test]
    fn the_ways_of_a_key_are_what_the_actions_carry() {
        let parsed = |mute: &str, hold: &str, one: &str| [mute, hold, one].map(|text| parse(text).unwrap());
        let keys = single_keys(&parsed("RightShift:tap", "RightShift:hold", "RightShift:double"), &[false; 3]);
        assert_eq!(
            keys,
            [(
                SingleKey::RightShift,
                Carried { hold: Some(Action::HoldToTalk), tap: Some(Action::Mute), double: Some(Action::OneTurn) }
            )]
        );
        assert_eq!(keys[0].1.ways(), Ways { hold: true, tap: true, double: true });
        assert_eq!(keys[0].1.action(Double), Some(Action::OneTurn));
        let two = single_keys(&parsed("Ctrl+Alt+M", "Fn:hold", "F9:double"), &[false; 3]);
        assert_eq!(two.len(), 2, "a combination is no single key");
        assert_eq!(two[0].0, SingleKey::Fn);
        assert_eq!(two[1].0, SingleKey::Function(9));
        // What is left out carries nothing.
        let left_out = single_keys(&parsed("RightShift:tap", "RightShift:hold", ""), &[true, false, false]);
        assert_eq!(
            left_out,
            [(SingleKey::RightShift, Carried { hold: Some(Action::HoldToTalk), ..Carried::default() })]
        );
    }

    #[test]
    fn what_a_listener_hears_reaches_only_the_keys_bound() {
        let bound = |mute: &str, hold: &str, one: &str| [mute, hold, one].map(|text| parse(text).unwrap());
        let bindings = bound("Ctrl+Alt+M", "Fn:hold", "RightControl:double");
        let fn_key = |down| Listened::Single { key: SingleKey::Fn, down };
        assert_eq!(route(&bindings, fn_key(true)), [(SingleKey::Fn, Edge::Down)]);
        assert_eq!(route(&bindings, fn_key(false)), [(SingleKey::Fn, Edge::Up)]);
        let control = Listened::Single { key: SingleKey::RightControl, down: true };
        assert_eq!(route(&bindings, control), [(SingleKey::RightControl, Edge::Down)]);
        assert!(route(&bindings, Listened::Single { key: SingleKey::RightAlt, down: true }).is_empty());
        // Another key spoils a pending tap of every key a listener carries, not of a combination's action.
        assert_eq!(
            route(&bindings, Listened::Other),
            [(SingleKey::Fn, Edge::Other), (SingleKey::RightControl, Edge::Other)]
        );
        assert!(
            route(&bound("Ctrl+Alt+M", "", "F13:tap"), Listened::Other).is_empty(),
            "a hot key's key hears no keys"
        );
        // One physical key carrying several ways is one key, fed once.
        let shared = bound("RightShift:tap", "RightShift:hold", "RightShift:double");
        let shift = |down| Listened::Single { key: SingleKey::RightShift, down };
        assert_eq!(route(&shared, shift(true)), [(SingleKey::RightShift, Edge::Down)]);
        assert_eq!(route(&shared, Listened::Other), [(SingleKey::RightShift, Edge::Other)]);
    }

    #[test]
    fn one_physical_key_serves_one_action_in_each_way() {
        let bound = |mute: &str, hold: &str, one: &str| [mute, hold, one].map(|text| parse(text).unwrap());
        // Hold to talk and say one thing on one key coexist; the same way twice does not, and the first holds it.
        assert_eq!(duplicates(&bound("", "RightShift:hold", "RightShift:double")), [None; 3]);
        assert_eq!(duplicates(&bound("RightShift:tap", "RightShift:hold", "RightShift:double")), [None; 3]);
        assert_eq!(duplicates(&bound("RightControl:tap", "", "RightControl:tap")), [None, None, Some(Action::Mute)]);
        assert_eq!(
            duplicates(&bound("RightControl:double", "", "RightControl:double")),
            [None, None, Some(Action::Mute)]
        );
        assert_eq!(duplicates(&bound("F13:tap", "", "F13:tap")), [None, None, Some(Action::Mute)]);
        // Other keys are not this one's business, nor are combinations (the registration diff sees those).
        assert_eq!(duplicates(&bound("RightControl:tap", "Fn:hold", "RightAlt:double")), [None; 3]);
        assert_eq!(duplicates(&bound("Ctrl+Alt+M", "", "Ctrl+Alt+M")), [None; 3]);
        // An F-key has one hot key: the later way rides on the earlier action's.
        let f = bound("F13:tap", "F13:hold", "F13:double");
        let none = duplicates(&f);
        assert_eq!(none, [None; 3]);
        assert_eq!(rides(&f, &none), [None, Some(Action::Mute), Some(Action::Mute)]);
        let twice = bound("F13:tap", "F14:hold", "F13:tap");
        let dup = duplicates(&twice);
        assert_eq!(dup, [None, None, Some(Action::Mute)]);
        assert_eq!(rides(&twice, &dup), [None; 3], "a duplicate rides on nothing");
        assert_eq!(
            rides(&bound("RightShift:tap", "RightShift:hold", ""), &[None; 3]),
            [None; 3],
            "a listener has no hot key"
        );
        // And the refused one is offered its own suggestion, if the system takes it.
        let mut fake = Fake::default();
        assert_eq!(alternatives_for(&mut fake, &wanted(None, None, None), Action::OneTurn, ""), ["Ctrl+Shift+F12"]);
        assert_eq!(
            alternatives_for(&mut fake, &wanted(None, None, None), Action::Mute, "Ctrl+Shift+F9"),
            Vec::<String>::new()
        );
        assert_eq!(
            alternatives_for(&mut fake, &wanted(None, None, Some("Ctrl+Shift+F11")), Action::HoldToTalk, ""),
            Vec::<String>::new()
        );
    }

    /// R17: the suggestions are common keys a keyboard has, one for each action, and none of them is a system shortcut.
    #[test]
    fn the_suggestions_are_common_keys_one_for_each_action() {
        let mut seen = Vec::new();
        for (action, suggestion) in Action::ALL.into_iter().zip(ALTERNATIVES) {
            assert_eq!(ALTERNATIVES[action.index()], suggestion);
            let parsed = parse(suggestion).unwrap().unwrap();
            assert_eq!(parsed.accelerator(), suggestion, "already in the kept form");
            let Parsed::Combo(keys) = &parsed else { panic!("a combination") };
            let function: u8 = keys.rsplit('+').next().unwrap().trim_start_matches('F').parse().unwrap();
            assert!((9..=12).contains(&function), "F9 to F12, never F13 to F19: {suggestion}");
            assert_ne!(function, 10, "F10 opens a menu bar on Windows and Linux");
            assert!(
                keys.starts_with("Ctrl+Shift+") && !keys.contains("Alt"),
                "{suggestion}: Control and Shift, never with Option on a Mac"
            );
            assert!(!seen.contains(&suggestion), "one for each action");
            seen.push(suggestion);
        }
    }

    #[test]
    fn a_default_is_the_systems_and_only_mute_has_one() {
        assert_eq!(default_of(Action::Mute, DEFAULT_MUTE_OTHER), DEFAULT_MUTE_OTHER);
        assert_eq!(default_of(Action::Mute, DEFAULT_MUTE_MACOS), DEFAULT_MUTE_MACOS);
        assert_eq!(default_of(Action::HoldToTalk, DEFAULT_MUTE_OTHER), "");
        assert_eq!(default_of(Action::OneTurn, DEFAULT_MUTE_MACOS), "");
    }

    #[test]
    fn a_hold_is_let_go_after_a_minute_a_sleep_a_lock_or_the_end_of_the_call() {
        let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mut watch = Watch::new(at(0), wall);
        assert_eq!(watch.look(at(1000), wall + Duration::from_secs(1), false), None);
        assert_eq!(
            watch.look(at(59_000), wall + Duration::from_secs(2), false),
            None,
            "the monotonic clock rules the minute"
        );
        assert_eq!(watch.look(at(60_000), wall + Duration::from_secs(3), false), Some(Release::TooLong));
        let mut slept = Watch::new(at(0), wall);
        assert_eq!(
            slept.look(at(1000), wall + Duration::from_secs(900), false),
            Some(Release::Slept),
            "the wall clock jumped"
        );
        let mut locked = Watch::new(at(0), wall);
        assert_eq!(locked.look(at(1000), wall + Duration::from_secs(1), true), Some(Release::Locked));
        let mut back = Watch::new(at(0), wall);
        assert_eq!(back.look(at(1000), wall - Duration::from_secs(60), false), None, "a clock set back is not a sleep");

        let mut hold = Hold::default();
        let first = hold.begin().unwrap();
        assert_eq!(hold.begin(), None, "one hold at a time");
        assert!(hold.is_current(first));
        assert!(hold.end() && !hold.end());
        assert!(!hold.is_current(first), "a watch of an ended hold has nothing to release");
        let second = hold.begin().unwrap();
        assert!(second != first && !hold.is_current(first) && hold.is_current(second));
    }

    #[derive(Default)]
    struct Fake {
        /// What the system holds: the app's own, and what another app holds.
        ours: Vec<String>,
        others: Vec<String>,
        /// The app lost track of a registration of its own: the system still has it.
        calls: Vec<String>,
    }

    impl Registrar for Fake {
        fn register(&mut self, hot_key: &str) -> bool {
            self.calls.push(format!("+{hot_key}"));
            if self.others.iter().chain(&self.ours).any(|k| k == hot_key) {
                return false;
            }
            self.ours.push(hot_key.to_owned());
            true
        }

        fn unregister(&mut self, hot_key: &str) -> bool {
            self.calls.push(format!("-{hot_key}"));
            let before = self.ours.len();
            self.ours.retain(|k| k != hot_key);
            self.ours.len() != before
        }
    }

    fn wanted(mute: Option<&str>, hold: Option<&str>, one: Option<&str>) -> [Option<String>; 3] {
        [mute, hold, one].map(|k| k.map(str::to_owned))
    }

    #[test]
    fn changing_one_shortcut_leaves_the_others_registered() {
        let (mut fake, mut held) = (Fake::default(), Held::default());
        let first = reconcile(&mut fake, &mut held, &wanted(Some("Ctrl+Alt+M"), Some("Ctrl+Alt+H"), None), true);
        assert_eq!(first[0].as_ref().unwrap().state, Registration::On);
        assert_eq!(first[1].as_ref().unwrap().state, Registration::On);
        assert!(first[2].is_none());
        fake.calls.clear();
        // Only mute changes: hold to talk is not touched, not even for a moment.
        let second = reconcile(&mut fake, &mut held, &wanted(Some("Ctrl+Alt+Shift+M"), Some("Ctrl+Alt+H"), None), true);
        assert_eq!(fake.calls, ["-Ctrl+Alt+M", "+Ctrl+Alt+Shift+M"]);
        assert_eq!(second[0].as_ref().unwrap().state, Registration::On);
        assert_eq!(second[1].as_ref().unwrap().state, Registration::On);
        assert_eq!(
            (held.of(Action::Mute), held.of(Action::HoldToTalk)),
            (Some("Ctrl+Alt+Shift+M"), Some("Ctrl+Alt+H"))
        );
        // The same again: nothing is asked of the system at all.
        fake.calls.clear();
        reconcile(&mut fake, &mut held, &wanted(Some("Ctrl+Alt+Shift+M"), Some("Ctrl+Alt+H"), None), true);
        assert!(fake.calls.is_empty(), "{:?}", fake.calls);
        // Cleared: let go.
        let cleared = reconcile(&mut fake, &mut held, &wanted(None, Some("Ctrl+Alt+H"), None), true);
        assert!(cleared[0].is_none() && held.of(Action::Mute).is_none());
        assert_eq!(fake.ours, ["Ctrl+Alt+H"]);
    }

    #[test]
    fn a_key_another_app_holds_is_refused_with_alternatives_the_system_takes() {
        let mut fake = Fake { others: vec!["Ctrl+Alt+Shift+M".into()], ..Fake::default() };
        let mut held = Held::default();
        let out = reconcile(&mut fake, &mut held, &wanted(Some("Ctrl+Alt+Shift+M"), None, None), true);
        let mute = out[0].as_ref().unwrap();
        assert_eq!(mute.state, Registration::Refused);
        assert_eq!(
            mute.alternatives,
            ["Ctrl+Shift+F9"],
            "its own suggestion, never the refused one, and nothing is left registered by the probe"
        );
        assert!(fake.ours.is_empty() && held.of(Action::Mute).is_none());
    }

    /// The Windows report: "another app already uses it", for good. The app had let go of its own registration in its books
    /// while the system still held it, so every later attempt was told the keys were taken. It lets go first, then asks again.
    #[test]
    fn a_registration_the_app_lost_track_of_does_not_keep_the_shortcut_refused() {
        let mut fake = Fake { ours: vec!["Ctrl+Alt+Shift+M".into()], ..Fake::default() };
        let mut held = Held::default();
        let out = reconcile(&mut fake, &mut held, &wanted(Some("Ctrl+Alt+Shift+M"), None, None), true);
        assert_eq!(out[0].as_ref().unwrap().state, Registration::On);
        assert_eq!(held.of(Action::Mute), Some("Ctrl+Alt+Shift+M"));
        assert_eq!(fake.ours, ["Ctrl+Alt+Shift+M"], "held once, not twice");
    }

    #[test]
    fn two_actions_on_the_same_keys_the_first_holds_them_and_the_other_is_refused() {
        let (mut fake, mut held) = (Fake::default(), Held::default());
        let out =
            reconcile(&mut fake, &mut held, &wanted(Some("Ctrl+Alt+Shift+M"), Some("Ctrl+Alt+Shift+M"), None), true);
        assert_eq!(out[0].as_ref().unwrap().state, Registration::On);
        let hold = out[1].as_ref().unwrap();
        assert_eq!(hold.state, Registration::Refused);
        assert_eq!(hold.holder, Some(Action::Mute), "it names who holds the keys");
        assert_eq!(hold.alternatives, ["Ctrl+Shift+F11"], "hold to talk's own suggestion, not the keys mute holds");
        assert_eq!(fake.ours, ["Ctrl+Alt+Shift+M"]);
        // Mute moves away: the other takes the keys it asked for, with no restart.
        let out = reconcile(&mut fake, &mut held, &wanted(Some("Ctrl+Alt+K"), Some("Ctrl+Alt+Shift+M"), None), true);
        assert_eq!(out[1].as_ref().unwrap().state, Registration::On);
        assert_eq!(held.of(Action::HoldToTalk), Some("Ctrl+Alt+Shift+M"));
    }

    #[test]
    fn without_a_backend_everything_asked_is_unavailable() {
        let (mut fake, mut held) = (Fake::default(), Held::default());
        let out = reconcile(&mut fake, &mut held, &wanted(Some("Ctrl+Alt+Shift+M"), None, Some("F9")), false);
        assert_eq!(out[0].as_ref().unwrap().state, Registration::Unavailable);
        assert!(out[1].is_none());
        assert_eq!(out[2].as_ref().unwrap().state, Registration::Unavailable);
        assert!(fake.calls.is_empty());
    }

    #[test]
    fn only_a_person_still_on_the_old_default_is_moved_once() {
        assert_eq!(migrated_mute("Ctrl+Alt+M", false, DEFAULT_MUTE_OTHER).as_deref(), Some(DEFAULT_MUTE_OTHER));
        assert_eq!(migrated_mute("Ctrl+Alt+M", false, DEFAULT_MUTE_MACOS), None, "macOS keeps ⌃⌥M");
        assert_eq!(migrated_mute("Ctrl+Alt+M", true, DEFAULT_MUTE_OTHER), None, "chosen again after the move");
        assert_eq!(migrated_mute("Cmd+Shift+M", false, DEFAULT_MUTE_OTHER), None, "a choice stays");
        assert_eq!(migrated_mute("", false, DEFAULT_MUTE_OTHER), None, "off stays off");
        assert_eq!(migrated_mute("alt+control+m", false, DEFAULT_MUTE_OTHER).as_deref(), Some(DEFAULT_MUTE_OTHER));
    }

    fn mods(command: bool, control: bool, alt: bool, shift: bool) -> Mods {
        Mods { command, control, alt, shift }
    }

    fn key(code: u16, character: &str, mods: Mods) -> Heard {
        Heard::Key { code, character: Some(character.into()), mods }
    }

    #[test]
    fn the_recorder_captures_a_combination_an_f_key_or_escape() {
        let mut r = Recorder::new(Action::Mute);
        assert_eq!(
            r.hear(key(46, "m", mods(false, true, true, true)), at(0)),
            Recorded::Done(Some("Ctrl+Alt+Shift+M".into()))
        );
        assert_eq!(
            r.hear(key(46, "m", mods(false, false, false, false)), at(0)),
            Recorded::Pending,
            "a letter alone is not global"
        );
        assert_eq!(r.hear(key(105, "", mods(false, false, false, false)), at(0)), Recorded::Done(Some("F13".into())));
        assert_eq!(
            r.hear(key(49, " ", mods(true, false, false, false)), at(0)),
            Recorded::Done(Some("Cmd+Space".into()))
        );
        assert_eq!(r.hear(key(53, "", mods(false, false, false, false)), at(0)), Recorded::Done(None));
        assert_eq!(
            r.hear(key(53, "", mods(true, false, false, false)), at(0)),
            Recorded::Pending,
            "Esc with a modifier is a key"
        );
    }

    /// The four keypad operators a mechanical keyboard sends with Cmd+Alt (macOS key codes 69, 78, 67 and 75): four
    /// shortcuts, each registered as its own physical key, none of them reported as another's duplicate.
    /// What each system registers, decided by `global-hotkey`'s own tables: pure, so every system is tested here.
    #[test]
    fn each_system_registers_only_the_keys_it_can_carry_as_themselves() {
        use KeyReach::*;
        use Platform::*;
        // The ISO key registers nowhere; nothing else is bound in its place.
        for platform in [Macos, Windows, Linux] {
            assert_eq!(key_reach(platform, "IntlBackslash"), Never, "{platform:?}");
            assert_eq!(hot_key_reach(platform, "Ctrl+Alt+IntlBackslash"), Never);
        }
        // Windows: NumpadEqual is VK_E and NumpadEnter is VK_RETURN in the backend; Ctrl+NumpadEqual must not be Ctrl+E.
        assert_eq!(hot_key_reach(Windows, "Ctrl+NumpadEqual"), Never);
        assert_eq!(hot_key_reach(Windows, "Ctrl+NumpadEnter"), Never);
        assert_eq!(hot_key_reach(Windows, "Ctrl+E"), Physical);
        assert_eq!(hot_key_reach(Windows, "Ctrl+Enter"), Physical);
        assert_eq!(hot_key_reach(Windows, "Cmd+Alt+NumpadAdd"), Physical);
        assert_eq!(hot_key_reach(Windows, "Cmd+Alt+BracketRight"), Physical);
        // macOS has all the keypad.
        assert_eq!(hot_key_reach(Macos, "Ctrl+NumpadEqual"), Physical);
        assert_eq!(hot_key_reach(Macos, "Ctrl+NumpadEnter"), Physical);
        assert_eq!(hot_key_reach(Macos, "Ctrl+BracketRight"), Physical);
        // X11: no keysym for those two; punctuation is by keysym, so by layout; letters, F-keys and the keypad operators are not.
        assert_eq!(hot_key_reach(Linux, "Ctrl+NumpadEqual"), Never);
        assert_eq!(hot_key_reach(Linux, "Ctrl+NumpadEnter"), Never);
        for key in [
            "Minus",
            "Equal",
            "Comma",
            "Period",
            "Slash",
            "Backslash",
            "Semicolon",
            "Quote",
            "BracketLeft",
            "BracketRight",
            "Backquote",
        ] {
            assert_eq!(hot_key_reach(Linux, &format!("Ctrl+{key}")), ByLayout, "{key}");
        }
        for hot in ["Ctrl+M", "F9", "Ctrl+Alt+NumpadAdd", "Ctrl+Numpad5", "Ctrl+Space"] {
            assert_eq!(hot_key_reach(Linux, hot), Physical, "{hot}");
        }
    }

    #[test]
    fn the_keypad_operators_are_four_different_shortcuts() {
        use global_hotkey::hotkey::{Code, HotKey, Modifiers};
        let cmd_alt = mods(true, false, true, false);
        let operators = [
            (69, "+", "Cmd+Alt+NumpadAdd", Code::NumpadAdd),
            (78, "-", "Cmd+Alt+NumpadSubtract", Code::NumpadSubtract),
            (67, "*", "Cmd+Alt+NumpadMultiply", Code::NumpadMultiply),
            (75, "/", "Cmd+Alt+NumpadDivide", Code::NumpadDivide),
        ];
        let mut recorded = Vec::new();
        for (code, character, accelerator, key) in operators {
            let done = Recorder::new(Action::Mute).hear(self::key(code, character, cmd_alt), at(0));
            assert_eq!(done, Recorded::Done(Some(accelerator.into())), "{character}");
            let hotkey: HotKey = accelerator.parse().unwrap();
            assert_eq!((hotkey.mods, hotkey.key), (Modifiers::SUPER | Modifiers::ALT, key), "{accelerator}");
            let parsed = parse(accelerator).unwrap().unwrap();
            assert_eq!(parsed.accelerator(), accelerator, "parses back");
            assert_eq!(parsed.hot_key().as_deref(), Some(accelerator));
            assert_eq!(label(accelerator, true, "en"), format!("⌘⌥Num {character}"));
            recorded.push(accelerator);
        }
        for (i, a) in recorded.iter().enumerate() {
            for (j, b) in recorded.iter().enumerate() {
                assert_eq!(settings::same_shortcut(a, b), i == j, "{a} / {b}");
            }
        }
        // The main row's keys with the same characters are other keys.
        for (code, character, accelerator) in
            [(24, "=", "Cmd+Alt+Equal"), (27, "-", "Cmd+Alt+Minus"), (44, "/", "Cmd+Alt+Slash")]
        {
            let done = Recorder::new(Action::Mute).hear(self::key(code, character, cmd_alt), at(0));
            assert_eq!(done, Recorded::Done(Some(accelerator.into())));
            assert!(!recorded.contains(&accelerator));
        }
    }

    /// A key is named by its code, whatever the layout types on it: a Spanish layout's `+` is on BracketRight.
    #[test]
    fn a_key_is_named_by_its_code_and_not_by_its_character() {
        assert_eq!(key_name(30, Some("+")).as_deref(), Some("BracketRight"));
        assert_eq!(key_name(46, Some("ñ")).as_deref(), Some("M"));
        assert_eq!(key_name(41, Some("ñ")).as_deref(), Some("Semicolon"));
        assert_eq!(key_name(82, Some("0")).as_deref(), Some("Numpad0"));
        assert_eq!(key_name(29, Some("0")).as_deref(), Some("0"));
        assert_eq!(key_name(76, Some("\r")).as_deref(), Some("NumpadEnter"));
        assert_eq!(key_name(36, Some("\r")).as_deref(), Some("Enter"));
        // A key with no code here is named by its character, but never by one the accelerator cannot carry.
        assert_eq!(key_name(110, Some("<")).as_deref(), Some("<"));
        assert_eq!(key_name(110, Some("+")), None);
        assert_eq!(key_name(110, Some(":")), None);
        assert_eq!(key_name(110, None), None);
        // The ISO key (`<>` beside the left Shift on Spanish, German and French keyboards) is its own key, never its character.
        assert_eq!(key_name(10, Some("<")).as_deref(), Some("IntlBackslash"));
        assert_eq!(key_name(10, Some("+")).as_deref(), Some("IntlBackslash"));
        assert_eq!(key_name(10, None).as_deref(), Some("IntlBackslash"));
    }

    #[test]
    fn the_recorder_captures_a_lone_key_the_way_its_action_allows() {
        let down = |key| Heard::Modifier { key, down: true };
        let up = |key| Heard::Modifier { key, down: false };
        // Hold to talk: held, ended by letting go.
        let mut hold = Recorder::new(Action::HoldToTalk);
        assert_eq!(hold.hear(down(SingleKey::Fn), at(0)), Recorded::Pending);
        assert_eq!(hold.hear(up(SingleKey::Fn), at(2000)), Recorded::Done(Some("Fn:hold".into())));
        // Mute: tapped.
        let mut mute = Recorder::new(Action::Mute);
        mute.hear(down(SingleKey::RightControl), at(0));
        assert_eq!(mute.hear(up(SingleKey::RightControl), at(100)), Recorded::Done(Some("RightControl:tap".into())));
        // Cmd+C is a combination, not a lone Command.
        let mut combo = Recorder::new(Action::Mute);
        combo.hear(down(SingleKey::RightCommand), at(0));
        assert_eq!(
            combo.hear(key(8, "c", mods(true, false, false, false)), at(50)),
            Recorded::Done(Some("Cmd+C".into()))
        );
        let mut spent = Recorder::new(Action::OneTurn);
        spent.hear(down(SingleKey::RightAlt), at(0));
        spent.hear(key(0, "a", mods(false, false, true, false)), at(10));
        assert_eq!(
            spent.hear(up(SingleKey::RightAlt), at(20)),
            Recorded::Pending,
            "a modifier of another key is not a lone key"
        );
        // One turn: a tap waits to see whether a second follows.
        let mut one = Recorder::new(Action::OneTurn);
        one.hear(down(SingleKey::RightShift), at(0));
        assert_eq!(one.hear(up(SingleKey::RightShift), at(80)), Recorded::Pending);
        assert_eq!(one.settle(at(300)), Recorded::Pending);
        assert_eq!(one.settle(at(481)), Recorded::Done(Some("RightShift:tap".into())));
        let mut two = Recorder::new(Action::OneTurn);
        two.hear(down(SingleKey::RightShift), at(0));
        two.hear(up(SingleKey::RightShift), at(80));
        two.hear(down(SingleKey::RightShift), at(200));
        assert_eq!(two.hear(up(SingleKey::RightShift), at(280)), Recorded::Done(Some("RightShift:double".into())));
    }

    #[test]
    fn macos_flags_tell_which_key_is_down() {
        // Right Command down with the left one up, as flagsChanged reports it: the device bit says which side.
        let right_command = 0x0010_0000 | 0x10;
        assert!(SingleKey::RightCommand.macos_down(right_command));
        assert!(!SingleKey::RightAlt.macos_down(right_command));
        let left_command = 0x0010_0000 | 0x08;
        assert!(!SingleKey::RightCommand.macos_down(left_command), "the left Command is not the right one");
        assert!(SingleKey::Fn.macos_down(0x0080_0000) && !SingleKey::Fn.macos_down(0));
        assert!(SingleKey::RightControl.macos_down(0x0004_0000 | 0x2000));
        assert!(SingleKey::RightShift.macos_down(0x0002_0000 | 0x04));
        assert!(!SingleKey::Function(13).macos_down(u64::MAX));
        let held = Mods::from_macos_flags(0x0004_0000 | 0x0008_0000 | 0x0002_0000);
        assert_eq!(held, Mods { command: false, control: true, alt: true, shift: true });
    }

    #[test]
    fn a_left_hand_modifier_is_told_from_the_right_one() {
        // Left Command down (device bit 0x08), then up with the right one still held (0x10).
        assert_eq!(SingleKey::left_macos_down(55, 0x0010_0000 | 0x08), Some(true));
        assert_eq!(SingleKey::left_macos_down(55, 0x0010_0000 | 0x10), Some(false), "only the right one is down");
        assert_eq!(SingleKey::left_macos_down(56, 0x0002_0000 | 0x02), Some(true));
        assert_eq!(SingleKey::left_macos_down(58, 0x0008_0000 | 0x20), Some(true));
        assert_eq!(SingleKey::left_macos_down(59, 0x0004_0000 | 0x01), Some(true));
        for right in [54, 60, 61, 62, 63] {
            assert_eq!(SingleKey::left_macos_down(right, u64::MAX), None, "{right}: a right-hand key or Fn");
            assert!(SingleKey::from_macos_modifier(right).is_some());
        }
        assert_eq!(SingleKey::left_macos_down(57, u64::MAX), None, "Caps Lock");
    }

    #[test]
    fn macos_modifier_keycodes_name_the_single_keys() {
        assert_eq!(SingleKey::from_macos_modifier(63), Some(SingleKey::Fn));
        assert_eq!(SingleKey::from_macos_modifier(61), Some(SingleKey::RightAlt));
        assert_eq!(SingleKey::from_macos_modifier(54), Some(SingleKey::RightCommand));
        assert_eq!(SingleKey::from_macos_modifier(62), Some(SingleKey::RightControl));
        assert_eq!(SingleKey::from_macos_modifier(60), Some(SingleKey::RightShift));
        assert_eq!(SingleKey::from_macos_modifier(58), None, "the left Option is not offered");
    }

    #[test]
    fn the_status_reads_as_the_page_expects() {
        use serde_json::json;
        let mute = GlobalShortcut {
            is_default: true,
            default: Some("Ctrl+Alt+M".into()),
            ..GlobalShortcut::new("Ctrl+Alt+M", Registration::On, vec![], true, "en")
        };
        let default_none = GlobalShortcut { is_default: true, ..GlobalShortcut::default() };
        let status = Status {
            shortcuts: Shortcuts { mute, hold_to_talk: default_none.clone(), one_turn: default_none },
            permission: Some(Permission { kind: PermissionKind::InputMonitoring, state: PermissionState::NotAsked }),
            headset: Some(Headset::new(HeadsetPress::OneTurn, HeadsetPress::Skip)),
        };
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            json!({
                "shortcuts": {
                    "mute": {"accelerator": "Ctrl+Alt+M", "label": "⌃⌥M", "state": "on", "alternatives": [], "voiceOver": false, "layoutBound": false, "managedBySystem": false, "isDefault": true, "default": "Ctrl+Alt+M", "refusal": null},
                    "holdToTalk": {"accelerator": null, "label": null, "state": "off", "alternatives": [], "voiceOver": false, "layoutBound": false, "managedBySystem": false, "isDefault": true, "default": null, "refusal": null},
                    "oneTurn": {"accelerator": null, "label": null, "state": "off", "alternatives": [], "voiceOver": false, "layoutBound": false, "managedBySystem": false, "isDefault": true, "default": null, "refusal": null},
                },
                "permission": {"kind": "inputMonitoring", "state": "notAsked"},
                "headset": {"press": "oneTurn", "doublePress": "skip", "outsideCall": "leave"},
            })
        );
    }

    /// R15: a modifier on its own that cannot be a key is ignored, the recording goes on, and the page is told why.
    #[test]
    fn a_lone_modifier_that_cannot_be_a_key_is_ignored_with_a_hint_and_the_recording_goes_on() {
        let left = |down| Heard::LeftModifier { down };
        let down = |key| Heard::Modifier { key, down: true };
        let up = |key| Heard::Modifier { key, down: false };
        // macOS: a left-hand modifier alone.
        let mut mac = Recorder::new(Action::Mute);
        assert_eq!(mac.hear(left(true), at(0)), Recorded::Pending);
        assert_eq!(mac.hear(left(false), at(80)), Recorded::Hint(Hint::LeftModifier));
        assert_eq!(mac.hear(left(false), at(90)), Recorded::Pending, "one hint for each press");
        assert_eq!(mac.settle(at(1000)), Recorded::Pending);
        // ...and then a real key is still taken.
        assert_eq!(
            mac.hear(key(46, "m", mods(false, true, true, false)), at(2000)),
            Recorded::Done(Some("Ctrl+Alt+M".into()))
        );
        // The left Command of a Cmd+C is a chord, not a lone key: no hint.
        let mut chord = Recorder::new(Action::Mute);
        chord.hear(left(true), at(0));
        assert_eq!(
            chord.hear(key(8, "c", mods(true, false, false, false)), at(40)),
            Recorded::Done(Some("Cmd+C".into()))
        );
        assert_eq!(chord.hear(left(false), at(60)), Recorded::Pending);
        let mut typed = Recorder::new(Action::Mute);
        typed.hear(left(true), at(0));
        typed.hear(key(0, "a", mods(false, false, false, true)), at(10));
        assert_eq!(typed.hear(left(false), at(20)), Recorded::Pending);
        // The right-hand ones still record on macOS.
        assert_eq!(
            {
                let mut right = Recorder::new(Action::Mute);
                right.hear(down(SingleKey::RightShift), at(0));
                right.hear(up(SingleKey::RightShift), at(50))
            },
            Recorded::Done(Some("RightShift:tap".into()))
        );
        // Windows and Linux: none of them, Fn included, is a key; the recording waits for a real key.
        for lone in [SingleKey::RightShift, SingleKey::RightControl, SingleKey::Fn] {
            let mut other = Recorder::on(Action::HoldToTalk, false);
            assert_eq!(other.hear(down(lone), at(0)), Recorded::Pending);
            assert_eq!(other.hear(up(lone), at(2000)), Recorded::Hint(Hint::LoneModifier), "{lone:?}");
            assert_eq!(other.settle(at(3000)), Recorded::Pending);
        }
        let mut other = Recorder::on(Action::Mute, false);
        other.hear(left(true), at(0));
        assert_eq!(other.hear(left(false), at(50)), Recorded::Hint(Hint::LoneModifier));
        assert_eq!(
            other.hear(key(105, "", mods(false, false, false, false)), at(100)),
            Recorded::Done(Some("F13".into())),
            "an F-key is a real key"
        );
        assert_eq!(serde_json::to_value(Hint::LeftModifier).unwrap(), "leftModifier");
        assert_eq!(serde_json::to_value(Hint::LoneModifier).unwrap(), "loneModifier");
    }

    #[test]
    fn a_refusal_names_who_holds_the_keys() {
        use serde_json::json;
        assert_eq!(
            serde_json::to_value(settings::Refusal::Duplicate { action: Action::OneTurn }).unwrap(),
            json!({"reason": "duplicate", "action": "oneTurn"})
        );
        assert_eq!(serde_json::to_value(settings::Refusal::Taken).unwrap(), json!({"reason": "taken"}));
    }
}
