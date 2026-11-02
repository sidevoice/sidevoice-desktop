//! What the system's audio interruptions do to the call (D-AUDIO-FOCUS), as a state machine that does nothing itself: it
//! takes what the system said and what the page reports of the microphone, and answers with the [`Effect`]s the call
//! (`call.rs`) carries out. So its decisions are tested without a phone.
//!
//! Interruptions that end (a phone call, an alarm: iOS `began`/`ended`, Android `lost-transient`/`gained`, and the system's
//! call put on hold and taken off it, CallKit's or Telecom's) pause the call at the first and resume it at the last end. Those that do not end by themselves (iOS `shouldResume` false or an audio
//! session that could not be taken back; Android a focus lost for good or refused) leave the call paused until the person
//! unmutes; on Android the focus is then asked for again, and the playback is released only once it is granted.
use sidevoice_mobile_plugin::FocusState;

/// What the call carries out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// The page mutes the microphone (its own mute, which it remembers as the system's).
    Interrupt,
    /// The page gives back the mute it took, if it is still the system's; `restore_mic` false leaves it muted.
    Resume { restore_mic: bool },
    /// The agent's playback is held (`true`) or released.
    Playback(bool),
}

/// Where an interruption comes from: the call can be taken away by both at once (an alarm on an audio focus that is
/// also lost), and resumes only when neither holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// iOS: the audio session was interrupted.
    Session,
    /// Android: the call's audio focus was lost.
    Focus,
    /// The system's call (CallKit, Telecom) was put on hold: another call took its place.
    System,
}

/// What an interruption changes of the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Pause,
    Resume,
}

/// Which interruptions are on.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Interrupted {
    session: bool,
    focus: bool,
    system: bool,
}

impl Interrupted {
    fn any(self) -> bool {
        self.session || self.focus || self.system
    }

    /// `source` is on or off now: what that changes of the call, if anything.
    fn set(&mut self, source: Source, on: bool) -> Option<Change> {
        let was = self.any();
        match source {
            Source::Session => self.session = on,
            Source::Focus => self.focus = on,
            Source::System => self.system = on,
        }
        match (was, self.any()) {
            (false, true) => Some(Change::Pause),
            (true, false) => Some(Change::Resume),
            _ => None,
        }
    }
}

/// What the system said, as the call takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// The audio was taken away.
    Began,
    /// It is back. `resume`: the call goes on by itself (the system advises it, and the audio could be taken back).
    Ended { resume: bool },
    /// The audio focus was refused outright: there is no end to wait for.
    Refused,
    /// The audio focus was lost for good (another app took it and plays): no gain will come.
    Lost,
}

/// What a change of the audio focus says of the call: another app playing quietly over it (`Duck`) says nothing.
pub fn focus_signal(state: FocusState) -> Option<Signal> {
    match state {
        FocusState::LostTransient => Some(Signal::Began),
        FocusState::Lost => Some(Signal::Lost),
        FocusState::Gained => Some(Signal::Ended { resume: true }),
        FocusState::Refused => Some(Signal::Refused),
        FocusState::Duck => None,
    }
}

/// Where the person's unmute is waited for after an interruption that does not end by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Wait {
    /// Whether the microphone has been seen muted: a report of the page from before the mute reached it is not an unmute.
    muted: bool,
}

impl Wait {
    /// The wait that begins with the microphone as the page last reported it (`enabled`, `disabled`): muted already if the
    /// page said so, whether before the interruption (the person's own mute) or since it began.
    fn of(mic: (bool, bool)) -> Wait {
        Wait { muted: !mic.0 }
    }
}

/// The page reports the microphone (`enabled`, `disabled`: the mute cannot be used): the wait after it, and whether the
/// person has lifted the pause. A microphone that cannot be muted has nothing to wait for.
fn wait_after(wait: Wait, enabled: bool, disabled: bool) -> (Option<Wait>, bool) {
    if disabled {
        return (None, true);
    }
    match (wait.muted, enabled) {
        (false, true) => (Some(wait), false),
        (false, false) | (true, false) => (Some(Wait { muted: true }), false),
        (true, true) => (None, true),
    }
}

/// What the interruptions have done to the call.
#[derive(Debug)]
pub struct Hold {
    interrupted: Interrupted,
    /// The wait for the person's unmute, when the pause does not end by itself.
    waiting: Option<Wait>,
    /// The wait is for the audio focus too (Android): it was lost for good or refused.
    regain: bool,
    /// The focus has been asked for again: the playback is released when it is granted.
    regaining: bool,
    /// Whether the playback is held.
    paused: bool,
    /// The microphone as the page last reported it (`enabled`, `disabled`), kept always: a mute reported before the wait
    /// begins is the evidence the wait needs (the page reports a change only once).
    mic: (bool, bool),
}

impl Default for Hold {
    fn default() -> Self {
        Hold::new()
    }
}

impl Hold {
    pub fn new() -> Hold {
        Hold {
            interrupted: Interrupted::default(),
            waiting: None,
            regain: false,
            regaining: false,
            paused: false,
            mic: (true, false),
        }
    }

    fn playback(&mut self, held: bool, out: &mut Vec<Effect>) {
        self.paused = held;
        out.push(Effect::Playback(held));
    }

    /// Paused with no end to wait for, whatever the page's mute is now: the person's unmute lifts it.
    fn stay_paused(&mut self, regain: bool, out: &mut Vec<Effect>) {
        out.push(Effect::Interrupt);
        self.playback(true, out);
        self.regain = regain;
        self.regaining = false;
        self.waiting = Some(Wait::of(self.mic));
    }

    /// The system said `signal` from `source`.
    pub fn signal(&mut self, source: Source, signal: Signal) -> Vec<Effect> {
        let mut out = Vec::new();
        match signal {
            Signal::Began => {
                self.regaining = false;
                if self.interrupted.set(source, true) == Some(Change::Pause) {
                    self.waiting = None;
                    self.regain = false;
                    out.push(Effect::Interrupt);
                    self.playback(true, &mut out);
                }
            }
            Signal::Ended { resume } => {
                if source == Source::Focus && self.regaining {
                    // The focus asked for again is granted: the person is back.
                    self.regaining = false;
                    self.regain = false;
                    self.waiting = None;
                    self.interrupted.set(Source::Focus, false);
                    if !self.interrupted.any() {
                        self.playback(false, &mut out);
                    }
                } else if self.interrupted.set(source, false) == Some(Change::Resume) {
                    if self.regain {
                        // The focus was lost for good meanwhile: still waiting for the person.
                    } else {
                        out.push(Effect::Resume {
                            restore_mic: resume,
                        });
                        if resume {
                            self.playback(false, &mut out);
                        } else {
                            self.waiting = Some(Wait::of(self.mic));
                        }
                    }
                }
            }
            Signal::Lost | Signal::Refused => {
                // Nothing will give it back: a transient loss that became permanent leaves the interruption it was.
                self.interrupted.set(source, false);
                self.stay_paused(true, &mut out);
            }
        }
        out
    }

    /// The page reports the microphone. `reactivate` gives the audio session back (false: refused), `refocus` asks the
    /// audio focus again (false: could not be asked); they are called only when the person lifts a wait.
    pub fn microphone(
        &mut self,
        enabled: bool,
        disabled: bool,
        reactivate: impl FnOnce() -> bool,
        refocus: impl FnOnce() -> bool,
    ) -> Vec<Effect> {
        self.mic = (enabled, disabled);
        let mut out = Vec::new();
        let Some(wait) = self.waiting else {
            return out;
        };
        let (waiting, lifted) = wait_after(wait, enabled, disabled);
        self.waiting = waiting;
        if !lifted {
            return out;
        }
        if !reactivate() {
            self.waiting = Some(Wait { muted: false });
        } else if self.regain {
            if refocus() {
                // The answer is a focus event: `Gained` releases the playback, anything else waits again.
                self.regaining = true;
            } else {
                self.waiting = Some(Wait { muted: false });
            }
        } else {
            self.playback(false, &mut out);
        }
        out
    }

    /// The call ends, or its page goes: whatever was held is let go. The page gives the mute back itself.
    pub fn clear(&mut self) -> Vec<Effect> {
        let mut out = Vec::new();
        let was_paused = self.paused;
        *self = Hold {
            mic: self.mic,
            ..Hold::new()
        };
        if was_paused {
            out.push(Effect::Playback(false));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_focus_pauses_the_call_and_gives_it_back_and_never_hangs_up() {
        assert_eq!(focus_signal(FocusState::Lost), Some(Signal::Lost));
        assert_eq!(focus_signal(FocusState::LostTransient), Some(Signal::Began));
        assert_eq!(
            focus_signal(FocusState::Gained),
            Some(Signal::Ended { resume: true })
        );
        assert_eq!(focus_signal(FocusState::Refused), Some(Signal::Refused));
        assert_eq!(focus_signal(FocusState::Duck), None);
    }

    #[test]
    fn a_pause_without_an_end_waits_for_the_person_to_unmute() {
        let waiting = Wait { muted: false };
        let muted = Wait { muted: true };
        // A report from before the mute reached the page is not an unmute.
        assert_eq!(wait_after(waiting, true, false), (Some(waiting), false));
        assert_eq!(wait_after(waiting, false, false), (Some(muted), false));
        assert_eq!(wait_after(muted, false, false), (Some(muted), false));
        assert_eq!(wait_after(muted, true, false), (None, true));
        // A microphone that cannot be muted has nothing to wait for.
        assert_eq!(wait_after(waiting, true, true), (None, true));
    }

    #[test]
    fn a_mute_reported_before_the_wait_begins_counts() {
        // Muted (by the interruption, or by the person before it), reported once; then the wait begins; then they unmute.
        let wait = Wait::of((false, false));
        assert_eq!(wait, Wait { muted: true });
        assert_eq!(wait_after(wait, true, false), (None, true));
        // Not yet muted when it begins: the unmute must come after the mute.
        let wait = Wait::of((true, false));
        assert_eq!(wait_after(wait, true, false), (Some(wait), false));
        let (wait, lifted) = wait_after(wait, false, false);
        assert!(!lifted);
        assert_eq!(wait_after(wait.unwrap(), true, false), (None, true));
    }

    #[test]
    fn the_call_pauses_at_the_first_interruption_and_resumes_at_the_last_end() {
        let mut now = Interrupted::default();
        assert_eq!(now.set(Source::Focus, false), None, "nothing was on");
        assert_eq!(now.set(Source::Focus, true), Some(Change::Pause));
        assert_eq!(now.set(Source::Focus, true), None, "already paused");
        assert_eq!(
            now.set(Source::Session, true),
            None,
            "paused by the other already"
        );
        assert_eq!(
            now.set(Source::Focus, false),
            None,
            "the session still holds it"
        );
        assert_eq!(now.set(Source::Session, false), Some(Change::Resume));
        assert_eq!(now, Interrupted::default());
    }

    const BEGAN: Signal = Signal::Began;
    const GAINED: Signal = Signal::Ended { resume: true };

    fn yes() -> bool {
        true
    }

    fn no() -> bool {
        false
    }

    fn holding() -> Vec<Effect> {
        vec![Effect::Interrupt, Effect::Playback(true)]
    }

    #[test]
    fn a_transient_loss_pauses_and_the_gain_resumes() {
        let mut hold = Hold::new();
        assert_eq!(hold.signal(Source::Focus, BEGAN), holding());
        assert_eq!(
            hold.signal(Source::Focus, GAINED),
            [
                Effect::Resume { restore_mic: true },
                Effect::Playback(false)
            ]
        );
    }

    #[test]
    fn a_permanent_loss_waits_for_the_person_and_asks_the_focus_again() {
        let mut hold = Hold::new();
        assert_eq!(hold.signal(Source::Focus, Signal::Lost), holding());
        // The mute reaches the page; the person unmutes: the session and the focus are asked for, the playback stays held.
        assert!(hold.microphone(false, false, yes, yes).is_empty());
        assert!(hold.microphone(true, false, yes, yes).is_empty());
        // Granted: released.
        assert_eq!(
            hold.signal(Source::Focus, GAINED),
            [Effect::Playback(false)]
        );
        assert!(hold
            .microphone(true, false, yes, || panic!("nothing is waiting"))
            .is_empty());
    }

    #[test]
    fn a_transient_loss_that_becomes_permanent_never_resumes_by_itself() {
        let mut hold = Hold::new();
        hold.signal(Source::Focus, BEGAN);
        let effects = hold.signal(Source::Focus, Signal::Lost);
        assert!(effects.contains(&Effect::Playback(true)));
        // Nothing comes from the system; a stray gain is the only thing that could, and the person is still waited for.
        assert!(hold.microphone(false, false, yes, yes).is_empty());
        assert!(hold.microphone(true, false, yes, yes).is_empty());
        assert_eq!(
            hold.signal(Source::Focus, GAINED),
            [Effect::Playback(false)]
        );
    }

    #[test]
    fn a_refused_request_again_pauses_again_and_waits_for_a_new_unmute() {
        let mut hold = Hold::new();
        hold.signal(Source::Focus, Signal::Refused);
        hold.microphone(false, false, yes, yes);
        hold.microphone(true, false, yes, yes); // asked again
                                                // Refused again: muted again, held, and the person must mute and unmute once more.
        assert_eq!(hold.signal(Source::Focus, Signal::Refused), holding());
        assert!(hold.microphone(true, false, yes, yes).is_empty());
        hold.microphone(false, false, yes, yes);
        hold.microphone(true, false, yes, yes);
        assert_eq!(
            hold.signal(Source::Focus, GAINED),
            [Effect::Playback(false)]
        );
    }

    #[test]
    fn a_delayed_grant_after_asking_again_is_a_transient_loss_then_a_gain() {
        let mut hold = Hold::new();
        hold.signal(Source::Focus, Signal::Lost);
        hold.microphone(false, false, yes, yes);
        hold.microphone(true, false, yes, yes);
        assert_eq!(hold.signal(Source::Focus, BEGAN), holding());
        assert_eq!(
            hold.signal(Source::Focus, GAINED),
            [
                Effect::Resume { restore_mic: true },
                Effect::Playback(false)
            ]
        );
    }

    #[test]
    fn a_focus_that_cannot_be_asked_for_waits_for_the_next_unmute() {
        let mut hold = Hold::new();
        hold.signal(Source::Focus, Signal::Lost);
        hold.microphone(false, false, yes, no);
        assert!(hold.microphone(true, false, yes, no).is_empty());
        hold.microphone(false, false, yes, yes);
        hold.microphone(true, false, yes, yes);
        assert_eq!(
            hold.signal(Source::Focus, GAINED),
            [Effect::Playback(false)]
        );
    }

    #[test]
    fn the_session_end_the_system_does_not_advise_waits_for_the_person() {
        let mut hold = Hold::new();
        hold.signal(Source::Session, BEGAN);
        hold.microphone(false, false, yes, yes); // muted, reported before the end
        assert_eq!(
            hold.signal(Source::Session, Signal::Ended { resume: false }),
            [Effect::Resume { restore_mic: false }]
        );
        // The unmute lifts it at once: the muted report was kept.
        assert_eq!(
            hold.microphone(true, false, yes, yes),
            [Effect::Playback(false)]
        );
    }

    #[test]
    fn the_audio_session_refused_again_keeps_the_playback_held() {
        let mut hold = Hold::new();
        hold.signal(Source::Session, BEGAN);
        hold.microphone(false, false, yes, yes);
        hold.signal(Source::Session, Signal::Ended { resume: false });
        assert!(hold.microphone(true, false, no, yes).is_empty());
        hold.microphone(false, false, yes, yes);
        assert_eq!(
            hold.microphone(true, false, yes, yes),
            [Effect::Playback(false)]
        );
    }

    #[test]
    fn a_person_already_muted_before_a_refusal_unmutes_to_lift_it() {
        let mut hold = Hold::new();
        hold.microphone(false, false, yes, yes); // muted by the person, reported once
        hold.signal(Source::Focus, Signal::Refused);
        hold.microphone(true, false, yes, yes);
        assert_eq!(
            hold.signal(Source::Focus, GAINED),
            [Effect::Playback(false)]
        );
    }

    #[test]
    fn a_microphone_that_cannot_be_muted_has_nothing_to_wait_for() {
        let mut hold = Hold::new();
        hold.microphone(true, true, yes, yes);
        hold.signal(Source::Session, BEGAN);
        hold.signal(Source::Session, Signal::Ended { resume: false });
        assert_eq!(
            hold.microphone(true, true, yes, yes),
            [Effect::Playback(false)]
        );
    }

    #[test]
    fn two_sources_pause_once_and_resume_at_the_last_end() {
        let mut hold = Hold::new();
        assert_eq!(hold.signal(Source::Session, BEGAN), holding());
        assert!(hold.signal(Source::Focus, BEGAN).is_empty());
        assert!(hold.signal(Source::Session, GAINED).is_empty());
        assert_eq!(
            hold.signal(Source::Focus, GAINED),
            [
                Effect::Resume { restore_mic: true },
                Effect::Playback(false)
            ]
        );
    }

    #[test]
    fn a_page_replaced_during_any_interruption_lets_the_playback_go() {
        // A transient interruption, a permanent-loss wait, and a focus asked for again that is not answered yet.
        let mut transient = Hold::new();
        transient.signal(Source::Focus, BEGAN);
        let mut permanent = Hold::new();
        permanent.signal(Source::Focus, Signal::Lost);
        let mut regaining = Hold::new();
        regaining.signal(Source::Focus, Signal::Lost);
        regaining.microphone(false, false, yes, yes);
        regaining.microphone(true, false, yes, yes);
        for mut hold in [transient, permanent, regaining] {
            assert_eq!(hold.clear(), [Effect::Playback(false)]);
        }
    }

    #[test]
    fn the_end_of_the_call_lets_the_playback_go_and_forgets_the_waits() {
        let mut hold = Hold::new();
        hold.signal(Source::Focus, Signal::Lost);
        assert_eq!(hold.clear(), [Effect::Playback(false)]);
        assert!(hold.clear().is_empty());
        assert!(hold.microphone(true, false, yes, yes).is_empty());
    }

    #[test]
    fn the_system_s_hold_pauses_the_call_and_taking_it_off_resumes_it() {
        let mut hold = Hold::new();
        assert_eq!(hold.signal(Source::System, BEGAN), holding());
        // The audio session interrupted under the hold changes nothing more, and its end resumes nothing alone.
        assert!(hold.signal(Source::Session, BEGAN).is_empty());
        assert!(hold.signal(Source::Session, GAINED).is_empty());
        assert_eq!(
            hold.signal(Source::System, GAINED),
            [
                Effect::Resume { restore_mic: true },
                Effect::Playback(false)
            ]
        );
    }
}
