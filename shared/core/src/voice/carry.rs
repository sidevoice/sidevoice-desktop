//! What the person said while the page that would hear it was gone (docs/BRIDGE.md → "A call that survives its page"). The
//! page of a live call can go without the person ending the call: it reloads, or the system ends the web view's process
//! (iOS does, under memory pressure). The calls' lanes belong to that page, so they are stopped and released when the next
//! page loads (`VoiceState::clear_older_than`), and nothing they say reaches a page. A turn of the person's under way then
//! would be lost with them, though they said it to a call they did not end.
//!
//! So a lane of a page that is gone is *carrying*: its open turn is closed and transcribed first, and the turns it
//! finishes, with words, are kept here, by the host, instead of being handed to the page. The next page asks for them
//! once it has joined the call again (`AppHost::carried`) and says them as turns of its own. Only turns with words are
//! kept, and only for [`KEPT_FOR`]: a call the person comes back to long after is not theirs to speak into any more.
//! Nothing else of the old call is kept (not the audio, not the meters, not a reply that was being said).

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use sidevoice_voice::TurnEvent;

/// How long a turn is kept for the next page to ask for it.
pub const KEPT_FOR: Duration = Duration::from_secs(10 * 60);

/// The most turns kept: a person does not speak more than this to a call with no page, and the oldest go first.
const MOST: usize = 8;

/// How long the lane of a page that is gone is waited for to finish the turn it had open: the turn is closed at once and
/// only its transcription is waited for.
pub const TRANSCRIPTION_WAIT: Duration = Duration::from_secs(8);

/// One turn of the person's, finished with its words, as the page gets it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CarriedTurn {
    pub turn_id: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// When the person started speaking and when the turn ended, in Unix milliseconds.
    pub started_at: u64,
    pub ended_at: u64,
    /// Whether the page that went had been told this turn started: the next page then says only its end, as words said
    /// while away, and not a second start of the same turn.
    pub started_told: bool,
    /// How long ago the turn ended when it was handed over, in milliseconds: a turn older than the core's resume window is
    /// said as words said while away, with that age.
    pub age_ms: u64,
}

impl CarriedTurn {
    /// The turn a lane's `event` ends with words, if it is one; the others (a start, a turn that came to nothing, one with
    /// no words in it) are nothing to carry.
    pub fn of(event: &TurnEvent) -> Option<CarriedTurn> {
        match event {
            TurnEvent::Finished { turn_id, text, language, started_at, ended_at, .. } if !text.trim().is_empty() => {
                Some(CarriedTurn {
                    turn_id: turn_id.clone(),
                    text: text.clone(),
                    language: language.clone(),
                    started_at: *started_at,
                    ended_at: *ended_at,
                    started_told: false,
                    age_ms: 0,
                })
            }
            _ => None,
        }
    }
}

/// A turn kept, and the loss of a page it belongs to (`None` until [`Carry::bind`] names it).
struct Kept {
    at: Instant,
    loss: Option<u64>,
    turn: CarriedTurn,
}

#[derive(Default)]
struct Held {
    turns: Vec<Kept>,
    /// The page loss the turns kept now belong to, and the conversation it was on (`rejoin::Rejoin`).
    loss: Option<(u64, Option<String>)>,
    /// No loss is waited for and none is coming: a turn kept now with no loss is a late one of a call that is over (its lane
    /// stops asynchronously), and is dropped instead of being adopted by the next loss. [`Carry::open`] ends it.
    closed: bool,
}

/// The turns kept for the next page, by the page loss and the conversation they were said in: only that loss's rejoin, on that
/// conversation, is handed them, and any other outcome (the window lapsed, the reload failed, the person joined another
/// conversation) drops them ([`Carry::settle`]).
#[derive(Default)]
pub struct Carry {
    held: Mutex<Held>,
}

impl Carry {
    /// Keeps `turn`, heard at `now`, for the loss bound now (or the next to be bound).
    pub fn keep(&self, turn: CarriedTurn, now: Instant) {
        let mut held = self.held.lock().unwrap();
        let loss = held.loss.as_ref().map(|(id, _)| *id);
        if loss.is_none() && held.closed {
            return;
        }
        held.turns.retain(|kept| now.saturating_duration_since(kept.at) < KEPT_FOR);
        held.turns.push(Kept { at: now, loss, turn });
        let most = held.turns.len().saturating_sub(MOST);
        held.turns.drain(..most);
    }

    /// The page was lost: loss `id`, on `conversation`. What was kept meanwhile is its, and so is what is kept from now on.
    pub fn bind(&self, id: u64, conversation: Option<String>) {
        let mut held = self.held.lock().unwrap();
        for kept in held.turns.iter_mut().filter(|kept| kept.loss.is_none()) {
            kept.loss = Some(id);
        }
        held.loss = Some((id, conversation));
        held.closed = false;
    }

    /// A page is being lost (the web view ended): what is finished from now on, until [`Carry::bind`] names the loss, is its.
    pub fn open(&self) {
        self.held.lock().unwrap().closed = false;
    }

    /// The turns of loss `id` kept for `conversation` that are still the person's to say, oldest first, each with its age at `now`; kept until [`Carry::settle`], so that a page lost again before it joined is handed them again.
    pub fn peek(&self, id: u64, conversation: Option<&str>, now: Instant) -> Vec<CarriedTurn> {
        let held = self.held.lock().unwrap();
        match &held.loss {
            Some((bound, on)) if *bound == id && on.as_deref() == conversation => held
                .turns
                .iter()
                .filter(|kept| kept.loss == Some(id))
                .map(|kept| (now.saturating_duration_since(kept.at), kept))
                .filter(|(age, _)| *age < KEPT_FOR)
                .map(|(age, kept)| CarriedTurn { age_ms: age.as_millis() as u64, ..kept.turn.clone() })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The loss `id` is over (the page that joined has the turns now, or the call ended, lapsed or went elsewhere): what was
    /// kept for it, and anything kept with no loss, is dropped. How many turns that was.
    pub fn settle(&self, id: u64) -> usize {
        let mut held = self.held.lock().unwrap();
        let before = held.turns.len();
        held.turns.retain(|kept| kept.loss.is_some_and(|loss| loss != id));
        if held.loss.as_ref().is_some_and(|(bound, _)| *bound == id) {
            held.loss = None;
        }
        held.closed = held.loss.is_none();
        before - held.turns.len()
    }

    /// Nothing is carried any more, whichever loss it was for (no call was remembered, or the person ended it). How many
    /// turns were dropped.
    pub fn clear(&self) -> usize {
        let mut held = self.held.lock().unwrap();
        held.loss = None;
        held.closed = true;
        std::mem::take(&mut held.turns).len()
    }
}

/// The turns of a lane that are open: started, and neither finished nor come to nothing yet.
#[derive(Default)]
pub struct OpenTurns {
    ids: Mutex<HashSet<String>>,
    /// The turns whose start was handed to a page (the page that went was told them).
    told: Mutex<HashSet<String>>,
}

impl OpenTurns {
    /// A step of a turn of the lane's call; `told`: it was handed to a page.
    pub fn seen(&self, event: &TurnEvent, told: bool) {
        let mut ids = self.ids.lock().unwrap();
        match event {
            TurnEvent::Started { turn_id, .. } => {
                ids.insert(turn_id.clone());
                if told {
                    self.told.lock().unwrap().insert(turn_id.clone());
                }
            }
            TurnEvent::Finished { turn_id, .. } | TurnEvent::Cancelled { turn_id, .. } => {
                ids.remove(turn_id);
            }
        }
    }

    /// Whether the start of `turn_id` was handed to a page; asked once, when the turn is kept.
    pub fn take_told(&self, turn_id: &str) -> bool {
        self.told.lock().unwrap().remove(turn_id)
    }

    pub fn any(&self) -> bool {
        !self.ids.lock().unwrap().is_empty()
    }

    /// Resolves once no turn is open, or when `limit` has passed (the turn is then lost with its call).
    pub async fn settled(&self, limit: Duration) -> bool {
        let until = tokio::time::Instant::now() + limit;
        while self.any() {
            if tokio::time::Instant::now() >= until {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        true
    }
}

#[cfg(test)]
mod tests;
