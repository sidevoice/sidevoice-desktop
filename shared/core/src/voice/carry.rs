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
                })
            }
            _ => None,
        }
    }
}

/// The turns kept for the next page.
#[derive(Default)]
pub struct Carry {
    turns: Mutex<Vec<(Instant, CarriedTurn)>>,
}

impl Carry {
    /// Keeps `turn`, heard at `now`.
    pub fn keep(&self, turn: CarriedTurn, now: Instant) {
        let mut turns = self.turns.lock().unwrap();
        turns.retain(|(at, _)| now.saturating_duration_since(*at) < KEPT_FOR);
        turns.push((now, turn));
        if turns.len() > MOST {
            let surplus = turns.len() - MOST;
            turns.drain(..surplus);
        }
    }

    /// Hands over the turns kept that are still the person's to say, oldest first, and keeps none: said once.
    pub fn take(&self, now: Instant) -> Vec<CarriedTurn> {
        let kept = std::mem::take(&mut *self.turns.lock().unwrap());
        kept.into_iter().filter(|(at, _)| now.saturating_duration_since(*at) < KEPT_FOR).map(|(_, turn)| turn).collect()
    }
}

/// The turns of a lane that are open: started, and neither finished nor come to nothing yet.
#[derive(Default)]
pub struct OpenTurns {
    ids: Mutex<HashSet<String>>,
}

impl OpenTurns {
    /// A step of a turn of the lane's call.
    pub fn seen(&self, event: &TurnEvent) {
        let mut ids = self.ids.lock().unwrap();
        match event {
            TurnEvent::Started { turn_id, .. } => {
                ids.insert(turn_id.clone());
            }
            TurnEvent::Finished { turn_id, .. } | TurnEvent::Cancelled { turn_id, .. } => {
                ids.remove(turn_id);
            }
        }
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
