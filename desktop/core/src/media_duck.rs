//! Lowering or pausing other apps' media while the person's microphone is open in a call, and giving back only what
//! the app itself took (docs/MEDIA_DUCK.md). Pure logic: what the system can do is observed and done by the desktop's
//! `platform::media_duck`, which hands this module what it saw as [`Player`]s and carries out the [`Step`]s it decides.

use serde::{Deserialize, Serialize};

/// What the person chose: lower the other apps' volume (the default), pause them, or leave them alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Lower,
    Pause,
    Off,
}

/// How far a lowered player goes: this fraction of the volume it had.
pub const LOWERED_TO: f32 = 0.25;
/// Volumes this close are the same: the system rounds what it stores.
const SAME: f32 = 0.03;
/// How many times giving back is tried, one per change of the call, before it is given up.
const RETRIES: u8 = 3;
/// How long a pause is owed: a call does not last longer than this without anyone touching the player.
pub const PAUSE_DEBT_SECS: u64 = 60 * 60;
/// How many players the app is owed for at most.
const MAX_OWED: usize = 32;

/// How this system can lower, as the page is told (`host.mediaDuck`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Lowering {
    /// Not at all.
    None,
    /// One app's volume at a time; the app's own sound is left alone.
    PerApp,
    /// The whole system's output volume, and only while an app this system can ask is playing: the app's own voice is
    /// lowered with the rest.
    System,
}

/// How far this system can pause, as the page is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pausing {
    None,
    /// Only some named apps.
    SomeApps,
    /// Any app that offers itself to the system's media controls.
    MediaControls,
}

/// Whether the system lets the app control the apps it pauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Access {
    Granted,
    /// The person said no (macOS: Privacy & Security -> Automation).
    Denied,
    /// Not asked yet, or no app to ask about is open.
    Undetermined,
}

/// What a player is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Playing,
    Paused,
    Stopped,
}

/// A media player as the system shows it now.
#[derive(Debug, Clone, PartialEq)]
pub struct Player {
    /// Whatever names it for the system, stable while it lives, and enough to find it again (its device included).
    pub key: String,
    /// The app it belongs to, as the system remembers a volume for it (empty when it does not): a volume set on one of its
    /// players is what its next one starts at.
    pub app: String,
    pub state: State,
    /// Its volume, 0 to 1, when it can be lowered on its own.
    pub volume: Option<f32>,
    pub pausable: bool,
}

/// What the system answers when asked for a player again.
#[derive(Debug, Clone, PartialEq)]
pub enum Seen {
    Is(Player),
    /// It, and the app it belonged to, are not there (now).
    Gone,
    /// The system could not be asked: a failure that may pass.
    Unknown,
}

/// One thing to do to a player.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    SetVolume { key: String, volume: f32 },
    Pause { key: String },
    Play { key: String },
}

/// What the app did to a player, kept to give it back: also across a crash (the desktop writes it down).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Touch {
    /// `from` is the volume it had, `to` the one it read back after the app set it. The system remembers a volume per app,
    /// so the player ending does not end the debt: its next one is found by `app` and given back too.
    Lowered {
        key: String,
        #[serde(default)]
        app: String,
        from: f32,
        to: f32,
    },
    Paused {
        key: String,
    },
}

impl Touch {
    /// Whether it is still worth giving back after a restart of the machine (`same_boot` false) or of `age_secs`: a pause
    /// is not (the player is not where it was, and a person who left it paused must stay so); a lowered volume the system
    /// stores is.
    pub fn outlives(&self, same_boot: bool, age_secs: u64) -> bool {
        match self {
            Touch::Lowered { .. } => true,
            Touch::Paused { .. } => same_boot && age_secs <= PAUSE_DEBT_SECS,
        }
    }
}

/// What the app has done to other apps' media, so it can undo exactly that and nothing else.
#[derive(Debug, Default)]
pub struct Ducker {
    open: Option<Mode>,
    touched: Vec<Touch>,
    retries: u8,
}

impl Ducker {
    /// A ducker that still owes what a run before it took and did not give back.
    pub fn owing(touched: Vec<Touch>) -> Self {
        Ducker { open: None, touched, retries: 0 }
    }

    /// What is owed now, for the desktop to keep.
    pub fn owed(&self) -> &[Touch] {
        &self.touched
    }

    /// Gives back what is owed, without taking anything: at start, for what an earlier run left.
    pub fn settle(&mut self, mut probe: impl FnMut(&str, &str) -> Seen, mut act: impl FnMut(&Step) -> bool) {
        self.give_back(&mut probe, &mut act);
    }

    /// Brings the media to what `want` asks (`None`: the microphone is closed, or the choice is to do nothing); nothing
    /// happens when it already is. `observe` reads the players, as far as that mode needs them; `probe` finds one again by
    /// its key and app (on its own device, not the default's); `act` does a step and says whether it worked. Only a step
    /// that worked is remembered. Giving back that fails is tried again at the next change, a few times; a player that is
    /// not there, or that cannot be asked now, keeps its debt for the next change. A change of mode gives back first.
    pub fn follow(
        &mut self,
        want: Option<Mode>,
        mut observe: impl FnMut(Mode) -> Vec<Player>,
        mut probe: impl FnMut(&str, &str) -> Seen,
        mut act: impl FnMut(&Step) -> bool,
    ) {
        if self.open == want {
            return;
        }
        self.give_back(&mut probe, &mut act);
        self.open = want;
        if let Some(mode) = want {
            for player in observe(mode) {
                if self
                    .touched
                    .iter()
                    .any(|t| matches!(t, Touch::Lowered { key, .. } | Touch::Paused { key } if *key == player.key))
                {
                    continue;
                }
                let Some(step) = Self::take(mode, &player) else { continue };
                if !act(&step) {
                    continue;
                }
                self.touched.push(match step {
                    Step::SetVolume { key, volume } => {
                        // What the system holds now, which it may have rounded: giving back checks against it.
                        let to = match probe(&key, &player.app) {
                            Seen::Is(p) => p.volume.unwrap_or(volume),
                            _ => volume,
                        };
                        Touch::Lowered { key, app: player.app.clone(), from: player.volume.unwrap_or(volume), to }
                    }
                    Step::Pause { key } | Step::Play { key } => Touch::Paused { key },
                });
            }
        }
    }

    /// What `mode` does to a player that is playing, if anything.
    fn take(mode: Mode, p: &Player) -> Option<Step> {
        if p.state != State::Playing {
            return None;
        }
        let key = p.key.clone();
        match (mode, p.volume) {
            (Mode::Lower, Some(from)) => Some(Step::SetVolume { key, volume: from * LOWERED_TO }),
            (Mode::Pause, _) if p.pausable => Some(Step::Pause { key }),
            _ => None,
        }
    }

    /// Gives back what was taken and still stands: a volume the person has changed since, a player that is playing again
    /// or has stopped, are left as they are.
    fn give_back(&mut self, probe: &mut impl FnMut(&str, &str) -> Seen, act: &mut impl FnMut(&Step) -> bool) {
        let mut failed = Vec::new();
        let mut kept = Vec::new();
        for touch in std::mem::take(&mut self.touched) {
            match &touch {
                Touch::Lowered { key, app, from, to } => match probe(key, app) {
                    // The player, or the app's next one, still at the volume the app set: give it back.
                    Seen::Is(p) if p.volume.is_some_and(|v| (v - to).abs() <= SAME) => {
                        if !act(&Step::SetVolume { key: p.key, volume: *from }) {
                            failed.push(touch);
                        }
                    }
                    Seen::Is(_) => {}
                    // Not playing now: the system remembers the volume, so its next player is found later.
                    Seen::Gone if !app.is_empty() => kept.push(touch),
                    Seen::Gone => {}
                    Seen::Unknown => kept.push(touch),
                },
                Touch::Paused { key } => match probe(key, "") {
                    Seen::Is(p) if p.state == State::Paused => {
                        if !act(&Step::Play { key: p.key }) {
                            failed.push(touch);
                        }
                    }
                    Seen::Unknown => kept.push(touch),
                    _ => {}
                },
            }
        }
        if failed.is_empty() {
            self.retries = 0;
        } else {
            self.retries += 1;
            // A few tries, one per change of the call; then it is given up.
            if self.retries < RETRIES {
                kept.extend(failed);
            } else {
                self.retries = 0;
            }
        }
        // The oldest go first when there are too many: an app that never comes back is not owed for ever.
        let excess = kept.len().saturating_sub(MAX_OWED);
        kept.drain(..excess);
        self.touched = kept;
    }
}

/// What the call asks of the media: the mode while the microphone is open in a call, none otherwise.
pub fn wanted(mode: Mode, joined: bool, mic_enabled: bool) -> Option<Mode> {
    (joined && mic_enabled && mode != Mode::Off).then_some(mode)
}

/// What `host.mediaDuck` answers: the choice, and how far this system can carry it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Status {
    pub mode: Mode,
    pub lower: Lowering,
    pub pause: Pausing,
    pub access: Access,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn player(key: &str, state: State, volume: Option<f32>, pausable: bool) -> Player {
        Player { key: key.into(), app: format!("app-{key}"), state, volume, pausable }
    }

    /// A fake system: players that steps change, as the real one does; `refuse` makes every step fail, `blind` makes
    /// every look fail.
    struct World {
        players: RefCell<Vec<Player>>,
        steps: RefCell<Vec<Step>>,
        refuse: RefCell<bool>,
        blind: RefCell<bool>,
    }

    impl World {
        fn new(players: Vec<Player>) -> Self {
            World {
                players: RefCell::new(players),
                steps: RefCell::default(),
                refuse: RefCell::default(),
                blind: RefCell::default(),
            }
        }
        fn run(&self, ducker: &mut Ducker, want: Option<Mode>) {
            ducker.follow(want, |_| self.players.borrow().clone(), |k, a| self.see(k, a), |step| self.act(step));
        }
        fn see(&self, key: &str, app: &str) -> Seen {
            if *self.blind.borrow() {
                return Seen::Unknown;
            }
            let players = self.players.borrow();
            // The player itself, else the app's next one: the system remembers a volume per app.
            match players
                .iter()
                .find(|p| p.key == key)
                .or_else(|| players.iter().find(|p| !app.is_empty() && p.app == app))
            {
                Some(p) => Seen::Is(p.clone()),
                None => Seen::Gone,
            }
        }
        fn act(&self, step: &Step) -> bool {
            self.steps.borrow_mut().push(step.clone());
            if *self.refuse.borrow() {
                return false;
            }
            let mut players = self.players.borrow_mut();
            match step {
                Step::SetVolume { key, volume } => {
                    players.iter_mut().find(|p| &p.key == key).unwrap().volume = Some(*volume)
                }
                Step::Pause { key } => players.iter_mut().find(|p| &p.key == key).unwrap().state = State::Paused,
                Step::Play { key } => players.iter_mut().find(|p| &p.key == key).unwrap().state = State::Playing,
            }
            true
        }
        fn get(&self, key: &str) -> Player {
            self.players.borrow().iter().find(|p| p.key == key).unwrap().clone()
        }
    }

    #[test]
    fn lowers_what_plays_and_gives_back_the_volume_it_had() {
        let world =
            World::new(vec![player("a", State::Playing, Some(0.8), true), player("b", State::Paused, Some(0.5), true)]);
        let mut ducker = Ducker::default();
        world.run(&mut ducker, Some(Mode::Lower));
        assert!((world.get("a").volume.unwrap() - 0.2).abs() < 1e-6);
        assert_eq!(world.get("b").volume, Some(0.5), "what is not playing is not touched");
        world.run(&mut ducker, None);
        assert_eq!(world.get("a").volume, Some(0.8));
        assert_eq!(world.steps.borrow().len(), 2);
        assert!(ducker.owed().is_empty());
    }

    #[test]
    fn pauses_what_plays_and_resumes_only_that() {
        let world = World::new(vec![
            player("playing", State::Playing, None, true),
            player("paused", State::Paused, None, true),
            player("stuck", State::Playing, None, false),
        ]);
        let mut ducker = Ducker::default();
        world.run(&mut ducker, Some(Mode::Pause));
        assert_eq!(world.get("playing").state, State::Paused);
        assert_eq!(world.get("stuck").state, State::Playing);
        world.run(&mut ducker, None);
        assert_eq!(world.get("playing").state, State::Playing);
        assert_eq!(world.get("paused").state, State::Paused, "never starts what was not playing");
    }

    #[test]
    fn a_player_that_was_stopped_meanwhile_is_not_started() {
        let world = World::new(vec![player("a", State::Playing, None, true)]);
        let mut ducker = Ducker::default();
        world.run(&mut ducker, Some(Mode::Pause));
        world.players.borrow_mut()[0].state = State::Stopped; // the person closed the track
        world.steps.borrow_mut().clear();
        world.run(&mut ducker, None);
        assert!(world.steps.borrow().is_empty());
    }

    #[test]
    fn leaves_alone_what_the_person_changed_meanwhile() {
        let world = World::new(vec![player("vol", State::Playing, Some(1.0), false)]);
        let mut ducker = Ducker::default();
        world.run(&mut ducker, Some(Mode::Lower));
        world.players.borrow_mut()[0].volume = Some(0.9); // the person raised it themselves
        world.run(&mut ducker, None);
        assert_eq!(world.get("vol").volume, Some(0.9));

        let world = World::new(vec![player("pau", State::Playing, None, true)]);
        let mut ducker = Ducker::default();
        world.run(&mut ducker, Some(Mode::Pause));
        world.players.borrow_mut()[0].state = State::Playing; // the person pressed play
        world.run(&mut ducker, None);
        assert_eq!(world.steps.borrow().len(), 1, "nothing is resumed twice");
    }

    #[test]
    fn a_step_that_failed_is_never_undone() {
        let world = World::new(vec![player("a", State::Playing, Some(1.0), true)]);
        let mut ducker = Ducker::default();
        *world.refuse.borrow_mut() = true;
        world.run(&mut ducker, Some(Mode::Lower));
        assert!(ducker.owed().is_empty());
        *world.refuse.borrow_mut() = false;
        world.players.borrow_mut()[0].volume = Some(LOWERED_TO);
        world.steps.borrow_mut().clear();
        world.run(&mut ducker, None);
        assert!(world.steps.borrow().is_empty());
    }

    #[test]
    fn giving_back_that_failed_is_tried_again_and_then_given_up() {
        let world = World::new(vec![player("a", State::Playing, Some(0.8), true)]);
        let mut ducker = Ducker::default();
        world.run(&mut ducker, Some(Mode::Lower));
        *world.refuse.borrow_mut() = true;
        world.run(&mut ducker, None);
        assert_eq!(ducker.owed().len(), 1, "kept for another try");
        *world.refuse.borrow_mut() = false;
        world.run(&mut ducker, Some(Mode::Off));
        world.run(&mut ducker, None);
        assert_eq!(world.get("a").volume, Some(0.8), "the next change gives it back");
        assert!(ducker.owed().is_empty());

        let mut ducker = Ducker::default();
        world.run(&mut ducker, Some(Mode::Lower));
        *world.refuse.borrow_mut() = true;
        for _ in 0..RETRIES {
            world.run(&mut ducker, None);
            world.run(&mut ducker, Some(Mode::Off));
        }
        assert!(ducker.owed().is_empty(), "not tried for ever");
    }

    #[test]
    fn a_player_that_cannot_be_asked_keeps_its_debt() {
        let world = World::new(vec![player("a", State::Playing, Some(0.8), true)]);
        let mut ducker = Ducker::default();
        world.run(&mut ducker, Some(Mode::Lower));
        *world.blind.borrow_mut() = true;
        for _ in 0..5 {
            world.run(&mut ducker, None);
            world.run(&mut ducker, Some(Mode::Off));
        }
        assert_eq!(ducker.owed().len(), 1, "a failure that may pass is not a reason to forget");
        *world.blind.borrow_mut() = false;
        world.run(&mut ducker, None);
        assert_eq!(world.get("a").volume, Some(0.8));
    }

    #[test]
    fn the_volume_the_system_remembers_for_an_app_is_given_back_to_its_next_player() {
        let world = World::new(vec![player("a", State::Playing, Some(0.8), false)]);
        let mut ducker = Ducker::default();
        world.run(&mut ducker, Some(Mode::Lower));
        // The stream ends, and the app's next one starts at the volume the system remembers.
        let mut next = player("a2", State::Playing, Some(0.2), false);
        next.app = "app-a".into();
        *world.players.borrow_mut() = vec![];
        world.run(&mut ducker, None);
        assert_eq!(ducker.owed().len(), 1, "the app is not there: still owed");
        world.players.borrow_mut().push(next);
        world.run(&mut ducker, Some(Mode::Off));
        world.run(&mut ducker, None);
        assert_eq!(world.get("a2").volume, Some(0.8));
        assert!(ducker.owed().is_empty());
    }

    #[test]
    fn the_volume_the_system_read_back_is_what_giving_back_checks() {
        // The system stores 0.2 as 0.3: a check against the asked volume would take it for the person's change.
        let world = World::new(vec![player("a", State::Playing, Some(0.92), false)]);
        let mut ducker = Ducker::default();
        ducker.follow(
            Some(Mode::Lower),
            |_| world.players.borrow().clone(),
            |key, _| match world.see(key, "") {
                Seen::Is(mut p) => {
                    p.volume = p.volume.map(|v| if v < 0.5 { 0.30 } else { v });
                    Seen::Is(p)
                }
                other => other,
            },
            |step| world.act(step),
        );
        assert!(matches!(ducker.owed(), [Touch::Lowered { to, .. }] if (*to - 0.30).abs() < 1e-6));
    }

    #[test]
    fn what_a_crash_left_is_given_back_by_the_next_run() {
        let world = World::new(vec![player("a", State::Playing, Some(0.8), true)]);
        let mut first = Ducker::default();
        world.run(&mut first, Some(Mode::Lower));
        let kept = serde_json::to_string(first.owed()).unwrap();
        let mut second = Ducker::owing(serde_json::from_str(&kept).unwrap());
        second.settle(|k, a| world.see(k, a), |step| world.act(step));
        assert_eq!(world.get("a").volume, Some(0.8));
        assert!(second.owed().is_empty());
    }

    #[test]
    fn a_pause_does_not_outlive_a_restart_or_an_hour_but_a_stored_volume_does() {
        let paused = Touch::Paused { key: "a".into() };
        let lowered = Touch::Lowered { key: "a".into(), app: "x".into(), from: 1.0, to: 0.25 };
        assert!(paused.outlives(true, 60));
        assert!(!paused.outlives(false, 60));
        assert!(!paused.outlives(true, PAUSE_DEBT_SECS + 1));
        assert!(lowered.outlives(false, 7 * 24 * 3600));
    }

    #[test]
    fn changing_the_mode_gives_back_before_it_takes() {
        let world = World::new(vec![player("a", State::Playing, Some(0.8), true)]);
        let mut ducker = Ducker::default();
        world.run(&mut ducker, Some(Mode::Lower));
        world.run(&mut ducker, Some(Mode::Pause));
        assert_eq!(world.get("a").state, State::Paused);
        assert_eq!(world.get("a").volume, Some(0.8));
        world.run(&mut ducker, None);
        assert_eq!(world.get("a").state, State::Playing);
        // Following the same want again does nothing.
        world.steps.borrow_mut().clear();
        world.run(&mut ducker, None);
        assert!(world.steps.borrow().is_empty());
    }

    #[test]
    fn the_call_wants_media_only_with_the_microphone_open_in_a_call() {
        assert_eq!(wanted(Mode::Lower, true, true), Some(Mode::Lower));
        assert_eq!(wanted(Mode::Pause, true, true), Some(Mode::Pause));
        assert_eq!(wanted(Mode::Off, true, true), None);
        assert_eq!(wanted(Mode::Lower, true, false), None);
        assert_eq!(wanted(Mode::Lower, false, true), None);
        assert_eq!(Mode::default(), Mode::Lower);
    }

    #[test]
    fn modes_are_written_as_the_page_writes_them() {
        assert_eq!(serde_json::to_string(&Mode::Pause).unwrap(), r#""pause""#);
        assert_eq!(serde_json::from_str::<Mode>(r#""off""#).unwrap(), Mode::Off);
        assert!(serde_json::from_str::<Mode>(r#""mute""#).is_err());
    }
}
