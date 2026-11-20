//! A call the page lost without the person ending it (docs/BRIDGE.md → "A call that survives its page"). The room page of a
//! phone can go while the person is in a call: it reloads, or the system ends the web view's process. The call is not the
//! person's to end then, so the app remembers it: which conversation it was on, and since when the page has been away. The
//! page that loads next is told to join that call again, once, if it comes back within [`WINDOW`].
//!
//! Only the page's own words end a call for good: a hang-up it reported (the call is no longer joined in its last report) or
//! one the app made natively from a surface (`hung_up`). Nothing here knows how the page rejoins, nor what the OS did to it.

use std::time::{Duration, Instant};

/// How long a call lost with its page is the person's to go back into. Past it the call is over (a person who comes back to
/// the app long after is not expecting a microphone that opens by itself).
pub const WINDOW: Duration = Duration::from_secs(10 * 60);

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

#[derive(Debug)]
struct Lost {
    conversation: Option<String>,
    at: Instant,
}

/// The call lost with its page, if any.
#[derive(Debug, Default)]
pub struct Rejoin {
    lost: Option<Lost>,
}

impl Rejoin {
    /// The page went. `live`: its last report had the call joined (or the call's service ran); `hung_up`: the app ended that
    /// call natively; `conversation`: the one the page last reported it was on.
    pub fn page_gone(&mut self, live: bool, hung_up: bool, conversation: Option<&str>, now: Instant) -> Gone {
        if hung_up {
            self.lost = None;
            return Gone::Ended;
        }
        if !live {
            return Gone::NoCall;
        }
        self.lost = Some(Lost { conversation: conversation.filter(|c| !c.is_empty()).map(str::to_owned), at: now });
        Gone::Remembered
    }

    /// A page reported (`ready`: its room is up; `joined`: it is in a call). The call lost is handed over once, as the
    /// conversation it was on (`Some(None)`: it had none), when this page is ready and in no call and the window is not
    /// over; a page that is in a call by itself has nothing to rejoin, and a window that lapsed ends the call for good.
    pub fn page_reported(&mut self, ready: bool, joined: bool, now: Instant) -> Option<Option<String>> {
        if !ready {
            return None;
        }
        let lost = self.lost.take()?;
        if joined || now.saturating_duration_since(lost.at) > WINDOW {
            return None;
        }
        Some(lost.conversation)
    }

    /// Whether a call lost waits for a page to join it.
    pub fn waiting(&self) -> bool {
        self.lost.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: fn() -> Instant = Instant::now;

    #[test]
    fn a_live_call_is_remembered_and_handed_to_the_next_ready_page_once() {
        let (mut rejoin, t) = (Rejoin::default(), NOW());
        assert_eq!(rejoin.page_gone(true, false, Some("thread"), t), Gone::Remembered);
        assert_eq!(rejoin.page_reported(false, false, t), None, "a page that is not up cannot rejoin");
        assert_eq!(rejoin.page_reported(true, false, t + Duration::from_secs(3)), Some(Some("thread".into())));
        assert_eq!(rejoin.page_reported(true, false, t + Duration::from_secs(4)), None, "once");
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
    fn a_page_that_loads_twice_before_it_is_ready_keeps_the_call() {
        let (mut rejoin, t) = (Rejoin::default(), NOW());
        rejoin.page_gone(true, false, Some("thread"), t);
        assert_eq!(rejoin.page_gone(false, false, None, t), Gone::NoCall, "the second page was never in the call");
        assert_eq!(rejoin.page_reported(true, false, t), Some(Some("thread".into())));
    }

    #[test]
    fn a_page_in_a_call_of_its_own_has_nothing_to_rejoin() {
        let (mut rejoin, t) = (Rejoin::default(), NOW());
        rejoin.page_gone(true, false, Some("thread"), t);
        assert_eq!(rejoin.page_reported(true, true, t), None);
        assert!(!rejoin.waiting());
    }

    #[test]
    fn a_call_lost_for_longer_than_the_window_is_over() {
        let (mut rejoin, t) = (Rejoin::default(), NOW());
        rejoin.page_gone(true, false, None, t);
        assert_eq!(rejoin.page_reported(true, false, t + WINDOW + Duration::from_secs(1)), None);
        assert!(!rejoin.waiting());
        rejoin.page_gone(true, false, Some(""), t);
        assert_eq!(rejoin.page_reported(true, false, t), Some(None), "no conversation named");
    }
}
