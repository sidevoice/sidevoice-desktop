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

/// How this system can lower, as the page is told (`host.mediaDuck`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Lowering {
    /// Not at all.
    None,
    /// One app's volume at a time; the app's own sound is left alone.
    PerApp,
    /// The whole system's output volume: the app's own voice is lowered with the rest.
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

/// A media player as the system shows it now.
#[derive(Debug, Clone, PartialEq)]
pub struct Player {
    /// Whatever names it for the system, stable while it lives.
    pub key: String,
    pub playing: bool,
    /// Its volume, 0 to 1, when it can be lowered on its own.
    pub volume: Option<f32>,
    pub pausable: bool,
}

/// One thing to do to a player.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    SetVolume { key: String, volume: f32 },
    Pause { key: String },
    Play { key: String },
}

#[derive(Debug, Clone, PartialEq)]
enum Touch {
    Lowered { key: String, from: f32, to: f32 },
    Paused { key: String },
}

/// What the app has done to other apps' media, so it can undo exactly that and nothing else.
#[derive(Debug, Default)]
pub struct Ducker {
    open: Option<Mode>,
    touched: Vec<Touch>,
}

impl Ducker {
    /// Brings the media to what `want` asks (`None`: the microphone is closed, or the choice is to do nothing). `observe`
    /// reads the players now, as far as that mode needs them; `act` does a step and says whether it worked. A step that failed is not remembered, so
    /// it is never undone; a change of mode undoes the old before it does the new.
    pub fn follow(
        &mut self,
        want: Option<Mode>,
        mut observe: impl FnMut(Mode) -> Vec<Player>,
        mut act: impl FnMut(&Step) -> bool,
    ) {
        if self.open == want {
            return;
        }
        if let Some(open) = self.open {
            for step in self.undo(&observe(open)) {
                act(&step);
            }
        }
        self.open = want;
        if let Some(mode) = want {
            for player in observe(mode) {
                if let Some((step, touch)) = Self::take(mode, &player) {
                    if act(&step) {
                        self.touched.push(touch);
                    }
                }
            }
        }
    }

    /// What `mode` does to a player that is playing, if anything.
    fn take(mode: Mode, p: &Player) -> Option<(Step, Touch)> {
        if !p.playing {
            return None;
        }
        let key = p.key.clone();
        match (mode, p.volume) {
            (Mode::Lower, Some(from)) => {
                let to = from * LOWERED_TO;
                Some((Step::SetVolume { key: key.clone(), volume: to }, Touch::Lowered { key, from, to }))
            }
            (Mode::Pause, _) if p.pausable => Some((Step::Pause { key: key.clone() }, Touch::Paused { key })),
            _ => None,
        }
    }

    /// The steps that give back what was taken and still stands: a volume the person has changed since, a player that
    /// is playing again or one that is gone, are left as they are.
    fn undo(&mut self, now: &[Player]) -> Vec<Step> {
        std::mem::take(&mut self.touched)
            .into_iter()
            .filter_map(|touch| match touch {
                Touch::Lowered { key, from, to } => {
                    let p = now.iter().find(|p| p.key == key)?;
                    p.volume.filter(|v| (v - to).abs() <= SAME).map(|_| Step::SetVolume { key, volume: from })
                }
                Touch::Paused { key } => {
                    let p = now.iter().find(|p| p.key == key)?;
                    (!p.playing).then_some(Step::Play { key })
                }
            })
            .collect()
    }
}

/// What `host.mediaDuck` answers: the choice, and how far this system can carry it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Status {
    pub mode: Mode,
    pub lower: Lowering,
    pub pause: Pausing,
}

/// What the call asks of the media: the mode while the microphone is open in a call, none otherwise.
pub fn wanted(mode: Mode, joined: bool, mic_enabled: bool) -> Option<Mode> {
    (joined && mic_enabled && mode != Mode::Off).then_some(mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn player(key: &str, playing: bool, volume: Option<f32>, pausable: bool) -> Player {
        Player { key: key.into(), playing, volume, pausable }
    }

    /// A fake system: players that steps change, as the real one does.
    struct World(RefCell<Vec<Player>>, RefCell<Vec<Step>>);

    impl World {
        fn new(players: Vec<Player>) -> Self {
            World(RefCell::new(players), RefCell::default())
        }
        fn run(&self, ducker: &mut Ducker, want: Option<Mode>) {
            ducker.follow(want, |_| self.0.borrow().clone(), |step| self.act(step));
        }
        fn act(&self, step: &Step) -> bool {
            self.1.borrow_mut().push(step.clone());
            let mut players = self.0.borrow_mut();
            match step {
                Step::SetVolume { key, volume } => {
                    players.iter_mut().find(|p| &p.key == key).unwrap().volume = Some(*volume)
                }
                Step::Pause { key } => players.iter_mut().find(|p| &p.key == key).unwrap().playing = false,
                Step::Play { key } => players.iter_mut().find(|p| &p.key == key).unwrap().playing = true,
            }
            true
        }
        fn get(&self, key: &str) -> Player {
            self.0.borrow().iter().find(|p| p.key == key).unwrap().clone()
        }
    }

    #[test]
    fn lowers_what_plays_and_gives_back_the_volume_it_had() {
        let world = World::new(vec![player("a", true, Some(0.8), true), player("b", false, Some(0.5), true)]);
        let mut ducker = Ducker::default();
        world.run(&mut ducker, Some(Mode::Lower));
        assert!((world.get("a").volume.unwrap() - 0.2).abs() < 1e-6);
        assert_eq!(world.get("b").volume, Some(0.5), "what is not playing is not touched");
        world.run(&mut ducker, None);
        assert_eq!(world.get("a").volume, Some(0.8));
        assert_eq!(world.1.borrow().len(), 2);
    }

    #[test]
    fn pauses_what_plays_and_resumes_only_that() {
        let world = World::new(vec![
            player("playing", true, None, true),
            player("paused", false, None, true),
            player("stuck", true, None, false),
        ]);
        let mut ducker = Ducker::default();
        world.run(&mut ducker, Some(Mode::Pause));
        assert!(!world.get("playing").playing && world.get("stuck").playing);
        world.run(&mut ducker, None);
        assert!(world.get("playing").playing);
        assert!(!world.get("paused").playing, "never starts what was not playing");
    }

    #[test]
    fn leaves_alone_what_the_person_changed_meanwhile() {
        let world = World::new(vec![player("vol", true, Some(1.0), true), player("pau", true, None, true)]);
        let mut ducker = Ducker::default();
        world.run(&mut ducker, Some(Mode::Lower));
        world.0.borrow_mut()[0].volume = Some(0.9); // the person raised it themselves
        world.run(&mut ducker, None);
        assert_eq!(world.get("vol").volume, Some(0.9));

        let mut ducker = Ducker::default();
        world.run(&mut ducker, Some(Mode::Pause));
        world.0.borrow_mut()[1].playing = true; // the person pressed play
        world.1.borrow_mut().clear();
        world.run(&mut ducker, None);
        assert!(
            world.1.borrow().iter().all(|s| !matches!(s, Step::Play { key } if key == "pau")),
            "nothing is resumed twice"
        );
    }

    #[test]
    fn a_player_that_left_is_forgotten() {
        let world = World::new(vec![player("a", true, Some(1.0), true)]);
        let mut ducker = Ducker::default();
        world.run(&mut ducker, Some(Mode::Lower));
        world.0.borrow_mut().clear();
        world.run(&mut ducker, None);
        assert_eq!(world.1.borrow().len(), 1);
    }

    #[test]
    fn a_step_that_failed_is_never_undone() {
        let world = World::new(vec![player("a", true, Some(1.0), true)]);
        let mut ducker = Ducker::default();
        ducker.follow(Some(Mode::Lower), |_| world.0.borrow().clone(), |_| false);
        world.0.borrow_mut()[0].volume = Some(LOWERED_TO);
        ducker.follow(None, |_| world.0.borrow().clone(), |_| panic!("nothing was taken"));
    }

    #[test]
    fn changing_the_mode_undoes_before_it_does() {
        let world = World::new(vec![player("a", true, Some(0.8), true)]);
        let mut ducker = Ducker::default();
        world.run(&mut ducker, Some(Mode::Lower));
        world.run(&mut ducker, Some(Mode::Pause));
        assert!(!world.get("a").playing);
        assert_eq!(world.get("a").volume, Some(0.8));
        world.run(&mut ducker, None);
        assert!(world.get("a").playing);
        // Following the same want again does nothing.
        world.1.borrow_mut().clear();
        world.run(&mut ducker, None);
        assert!(world.1.borrow().is_empty());
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
