//! A call the page lost without the person ending it (docs/BRIDGE.md → "A call that survives its page"). The room page of a
//! phone can go while the person is in a call: it reloads, or the system ends the web view's process. The call is not the
//! person's to end then, so the app remembers it: which conversation it was on, and since when the page has been away. The
//! page that loads next is told to join that call again, once, if it comes back within [`WINDOW`], and has [`JOIN_WAIT`]
//! to be in a call. Until one of those ends the loss, the call's native side (service, Live Activity, CallKit/Telecom) stays
//! as it was: the person's call does not look hung up for the seconds the page is away.
//!
//! Only the page's own words end a call for good: a hang-up it reported (the call is no longer joined in its last report) or
//! one the app made natively from a surface (`hung_up`, [`Rejoin::cancel`]), which also covers a hang-up made while the page
//! is away. Nothing here knows how the page rejoins, nor what the OS did to it.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long a call lost with its page is the person's to go back into. Past it the call is over (a person who comes back to
/// the app long after is not expecting a microphone that opens by itself).
pub const WINDOW: Duration = Duration::from_secs(10 * 60);

/// How long a page that was told to join the call again has to be in a call (the join itself, which the page carries on by
/// reconnecting for as long as it needs, counts as being in one). Past it the rejoin failed: the call is over.
pub const JOIN_WAIT: Duration = Duration::from_secs(45);

/// What became of a call when its page went, for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gone {
    /// The call was live and not ended by the person: the next page is to join it again.
    Remembered,
    /// The page was in no call; a call remembered from an earlier page stays as it was.
    NoCall,
    /// The person ended the call (a surface's hang-up); nothing is remembered.
    Ended,
}

/// What a page's report makes of the loss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    /// Nothing to do.
    Nothing,
    /// Tell this page to join `conversation` again (loss `id`): once. Also when the person already joined by themselves on
    /// the same conversation: the page merges what was carried into that call.
    Hand { id: u64, conversation: Option<String> },
    /// The page is in a call; the loss is over and what was carried is the page's now. `same`: it is on the conversation
    /// the call was on (on another one, the carried turns are not for it).
    Joined { id: u64, same: bool },
    /// The loss ended with no rejoin: the window lapsed, or the page never got into a call. Nothing carried is kept.
    Lapsed { id: u64 },
}

#[derive(Debug)]
struct Lost {
    id: u64,
    conversation: Option<String>,
    at: Instant,
    /// When the page was told to join (once).
    handed: Option<Instant>,
}

/// The call lost with its page, if any.
#[derive(Debug, Default)]
pub struct Rejoin {
    lost: Option<Lost>,
    last_id: u64,
}

impl Rejoin {
    /// The page went. `live`: its last report had the call joined (or the call's service ran); `hung_up`: the app ended that
    /// call natively; `conversation`: the one the page last reported it was on. A page that was lost again before it joined
    /// keeps the loss it was told about (same id), told again to the page after.
    pub fn page_gone(&mut self, live: bool, hung_up: bool, conversation: Option<&str>, now: Instant) -> Gone {
        if hung_up {
            self.lost = None;
            return Gone::Ended;
        }
        if !live {
            if let Some(lost) = self.lost.as_mut() {
                lost.handed = None;
            }
            return Gone::NoCall;
        }
        self.last_id += 1;
        self.lost = Some(Lost {
            id: self.last_id,
            conversation: conversation.filter(|c| !c.is_empty()).map(str::to_owned),
            at: now,
            handed: None,
        });
        Gone::Remembered
    }

    /// A page reported (`ready`: its room is up; `joined`: it is in a call, on `conversation`).
    pub fn page_reported(&mut self, ready: bool, joined: bool, conversation: Option<&str>, now: Instant) -> Report {
        if let Some(id) = self.expire(now) {
            return Report::Lapsed { id };
        }
        let Some(lost) = self.lost.as_mut() else { return Report::Nothing };
        if !ready {
            return Report::Nothing;
        }
        let same =
            lost.conversation.is_none() || lost.conversation.as_deref() == conversation.filter(|c| !c.is_empty());
        if lost.handed.is_some() {
            if !joined {
                return Report::Nothing;
            }
            let id = lost.id;
            self.lost = None;
            return Report::Joined { id, same };
        }
        if joined && !same {
            // The person joined another conversation by themselves: what was carried is not for it.
            let id = lost.id;
            self.lost = None;
            return Report::Joined { id, same: false };
        }
        lost.handed = Some(now);
        Report::Hand { id: lost.id, conversation: lost.conversation.clone() }
    }

    /// The loss that lapsed by `now`, if one did, forgotten: the window ran out before a page was ready, or the page told to
    /// join was in no call after [`JOIN_WAIT`].
    pub fn expire(&mut self, now: Instant) -> Option<u64> {
        let lost = self.lost.as_ref()?;
        let over = match lost.handed {
            Some(handed) => now.saturating_duration_since(handed) > JOIN_WAIT,
            None => now.saturating_duration_since(lost.at) > WINDOW,
        };
        over.then(|| self.lost.take().map(|lost| lost.id)).flatten()
    }

    /// The person ended the call (a hang-up from a surface, the system's call, a headset or a car): nothing is remembered.
    /// The id of the loss that was waiting, if any.
    pub fn cancel(&mut self) -> Option<u64> {
        self.lost.take().map(|lost| lost.id)
    }

    /// Whether a call lost waits for a page to join it: its native side stays up meanwhile.
    pub fn waiting(&self) -> bool {
        self.lost.is_some()
    }

    /// The loss waiting, by id and the conversation it was on.
    pub fn current(&self) -> Option<(u64, Option<&str>)> {
        self.lost.as_ref().map(|lost| (lost.id, lost.conversation.as_deref()))
    }
}

/// How many times in [`RELOAD_WINDOW`] the app loads a page again by itself.
pub const RELOAD_MAX: usize = 3;
pub const RELOAD_WINDOW: Duration = Duration::from_secs(2 * 60);
const RELOAD_BACKOFF: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(5)];

/// What to do about a web view that ended and needs loading again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reload {
    /// Now (the first in the window).
    Now,
    /// After this long (the next ones: a page that goes again at once is not loaded again at once).
    After(Duration),
    /// Not by itself: the page keeps going, so the call is given up and the page loaded once more `retry` from now, which
    /// is when the oldest reload leaves the window.
    Stop { retry: Duration },
}

/// The reloads the app made after the system ended the web view, for [`RELOAD_MAX`] in [`RELOAD_WINDOW`].
#[derive(Debug, Default)]
pub struct Reloads {
    made: VecDeque<Instant>,
}

impl Reloads {
    pub const fn new() -> Reloads {
        Reloads { made: VecDeque::new() }
    }

    /// A reload is wanted at `now`: what to do. An allowed one counts from `now` (or from after its backoff).
    pub fn next(&mut self, now: Instant) -> Reload {
        while self.made.front().is_some_and(|at| now.saturating_duration_since(*at) >= RELOAD_WINDOW) {
            self.made.pop_front();
        }
        let n = self.made.len();
        if n >= RELOAD_MAX {
            let oldest = *self.made.front().expect("n is not zero");
            return Reload::Stop { retry: (oldest + RELOAD_WINDOW).saturating_duration_since(now) };
        }
        let wait = n.checked_sub(1).map(|i| RELOAD_BACKOFF[i.min(RELOAD_BACKOFF.len() - 1)]).unwrap_or_default();
        self.made.push_back(now + wait);
        if wait.is_zero() {
            Reload::Now
        } else {
            Reload::After(wait)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: fn() -> Instant = Instant::now;
    const S: fn(u64) -> Duration = Duration::from_secs;

    #[test]
    fn a_live_call_is_remembered_and_handed_to_the_next_ready_page_once() {
        let (mut rejoin, t) = (Rejoin::default(), NOW());
        assert_eq!(rejoin.page_gone(true, false, Some("thread"), t), Gone::Remembered);
        assert_eq!(rejoin.page_reported(false, false, None, t), Report::Nothing, "a page that is not up cannot rejoin");
        assert_eq!(
            rejoin.page_reported(true, false, None, t + S(3)),
            Report::Hand { id: 1, conversation: Some("thread".into()) }
        );
        assert_eq!(rejoin.page_reported(true, false, None, t + S(4)), Report::Nothing, "once");
        assert!(rejoin.waiting(), "until the page is in a call");
        assert_eq!(rejoin.page_reported(true, true, Some("thread"), t + S(5)), Report::Joined { id: 1, same: true });
        assert!(!rejoin.waiting());
    }

    #[test]
    fn a_call_the_person_ended_is_not_remembered() {
        let (mut rejoin, t) = (Rejoin::default(), NOW());
        assert_eq!(rejoin.page_gone(true, false, Some("a"), t), Gone::Remembered);
        assert_eq!(rejoin.page_gone(true, true, Some("a"), t), Gone::Ended);
        assert!(!rejoin.waiting());
        assert_eq!(rejoin.page_gone(false, false, None, t), Gone::NoCall);
        assert!(!rejoin.waiting(), "a page in no call remembers nothing");
    }

    #[test]
    fn a_hang_up_while_the_page_is_away_forgets_the_call() {
        let (mut rejoin, t) = (Rejoin::default(), NOW());
        rejoin.page_gone(true, false, Some("thread"), t);
        assert_eq!(rejoin.cancel(), Some(1));
        assert_eq!(rejoin.page_reported(true, false, None, t + S(1)), Report::Nothing, "no unwanted rejoin");
        assert_eq!(rejoin.cancel(), None);
    }

    #[test]
    fn a_page_that_loads_twice_before_it_is_ready_keeps_the_call() {
        let (mut rejoin, t) = (Rejoin::default(), NOW());
        rejoin.page_gone(true, false, Some("thread"), t);
        assert_eq!(rejoin.page_gone(false, false, None, t), Gone::NoCall, "the second page was never in the call");
        assert_eq!(
            rejoin.page_reported(true, false, None, t),
            Report::Hand { id: 1, conversation: Some("thread".into()) }
        );
        // The page told to join went before it did: the next one is told again, with the same loss.
        assert_eq!(rejoin.page_gone(false, false, None, t + S(1)), Gone::NoCall);
        assert_eq!(
            rejoin.page_reported(true, false, None, t + S(2)),
            Report::Hand { id: 1, conversation: Some("thread".into()) }
        );
    }

    #[test]
    fn a_page_that_joined_the_same_conversation_itself_gets_what_was_carried() {
        let (mut rejoin, t) = (Rejoin::default(), NOW());
        rejoin.page_gone(true, false, Some("a"), t);
        assert_eq!(
            rejoin.page_reported(true, true, Some("a"), t),
            Report::Hand { id: 1, conversation: Some("a".into()) }
        );
        assert_eq!(rejoin.page_reported(true, true, Some("a"), t), Report::Joined { id: 1, same: true });
    }

    #[test]
    fn a_page_that_joined_another_conversation_leaves_nothing_to_carry() {
        let (mut rejoin, t) = (Rejoin::default(), NOW());
        rejoin.page_gone(true, false, Some("a"), t);
        assert_eq!(rejoin.page_reported(true, true, Some("b"), t), Report::Joined { id: 1, same: false });
        assert!(!rejoin.waiting());
        rejoin.page_gone(true, false, Some("a"), t);
        rejoin.page_reported(true, false, None, t);
        assert_eq!(rejoin.page_reported(true, true, Some("b"), t), Report::Joined { id: 2, same: false });
    }

    #[test]
    fn a_call_lost_for_longer_than_the_window_is_over() {
        let (mut rejoin, t) = (Rejoin::default(), NOW());
        rejoin.page_gone(true, false, None, t);
        assert_eq!(rejoin.page_reported(true, false, None, t + WINDOW + S(1)), Report::Lapsed { id: 1 });
        assert!(!rejoin.waiting());
        rejoin.page_gone(true, false, Some(""), t);
        assert_eq!(
            rejoin.page_reported(true, false, None, t),
            Report::Hand { id: 2, conversation: None },
            "no conversation named"
        );
    }

    #[test]
    fn a_page_told_to_join_that_is_in_no_call_after_the_wait_ends_the_loss() {
        let (mut rejoin, t) = (Rejoin::default(), NOW());
        rejoin.page_gone(true, false, Some("a"), t);
        rejoin.page_reported(true, false, None, t);
        assert_eq!(rejoin.expire(t + JOIN_WAIT), None);
        assert_eq!(rejoin.expire(t + JOIN_WAIT + S(1)), Some(1));
        assert!(!rejoin.waiting());
        // And a window that runs out with no page ready ends it with no report at all.
        rejoin.page_gone(true, false, None, t);
        assert_eq!(rejoin.expire(t + WINDOW + S(1)), Some(2));
    }

    #[test]
    fn reloads_back_off_and_stop_after_the_cap_until_the_window_clears() {
        let (mut reloads, t) = (Reloads::new(), NOW());
        assert_eq!(reloads.next(t), Reload::Now);
        assert_eq!(reloads.next(t + S(1)), Reload::After(S(2)));
        assert_eq!(reloads.next(t + S(4)), Reload::After(S(5)));
        assert_eq!(reloads.next(t + S(10)), Reload::Stop { retry: RELOAD_WINDOW - S(10) });
        assert_eq!(reloads.next(t + RELOAD_WINDOW), Reload::After(S(5)), "the window moved on: two left in it");
    }
}
