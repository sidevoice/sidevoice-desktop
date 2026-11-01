//! Pausing what the calls play, for as long as the system has the audio taken away (a phone call, an alarm, another app
//! playing).
//!
//! A pause holds back what the calls have yet to hand to the speaker: the chunks of what is being said are kept, in order,
//! and played when the pause ends. What the speaker already has queued is the device's own and is not touched here.
//! Nothing is dropped: a reply that was being said goes on from the next chunk, and a call stopped or cut off meanwhile
//! drops what it held, as it drops what it had queued.
use std::sync::{Arc, Mutex, Weak};

use sidevoice_voice::{AudioIo, IoSink};

#[cfg(test)]
mod tests;

/// Whether the host's calls are paused, and the lanes it pauses.
#[derive(Default)]
pub(super) struct PlaybackPause {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    paused: bool,
    lanes: Vec<Weak<Mutex<Gated>>>,
}

/// One chunk of something said, as `AudioIo::play` takes it.
struct Chunk {
    utterance: String,
    chunk: usize,
    samples: Vec<f32>,
    sample_rate: u32,
}

/// A lane's audio and what a pause holds back from it.
struct Gated {
    inner: Box<dyn AudioIo>,
    paused: bool,
    held: Vec<Chunk>,
}

impl Gated {
    /// Plays what was held, in order, and plays on from now.
    fn flush(&mut self) {
        self.paused = false;
        for Chunk { utterance, chunk, samples, sample_rate } in std::mem::take(&mut self.held) {
            self.inner.play(&utterance, chunk, samples, sample_rate);
        }
    }
}

impl PlaybackPause {
    /// `io`, gated by this pause: it is paused at once if the host is.
    pub(super) fn wrap(&self, io: Box<dyn AudioIo>) -> Box<dyn AudioIo> {
        let mut state = self.state.lock().unwrap();
        let gated = Arc::new(Mutex::new(Gated { inner: io, paused: state.paused, held: Vec::new() }));
        state.lanes.retain(|lane| lane.strong_count() > 0);
        state.lanes.push(Arc::downgrade(&gated));
        Box::new(Lane { gated })
    }

    /// Pauses (`true`) or resumes every lane, those made later too. Pausing a paused host, or resuming one that plays,
    /// does nothing.
    pub(super) fn set(&self, paused: bool) {
        let mut state = self.state.lock().unwrap();
        if state.paused == paused {
            return;
        }
        state.paused = paused;
        state.lanes.retain(|lane| lane.strong_count() > 0);
        for gated in state.lanes.iter().filter_map(Weak::upgrade) {
            let mut gated = gated.lock().unwrap();
            if paused {
                gated.paused = true;
            } else {
                gated.flush();
            }
        }
    }
}

/// The lane's audio as the call holds it.
struct Lane {
    gated: Arc<Mutex<Gated>>,
}

impl AudioIo for Lane {
    fn start(&mut self, sink: IoSink, input: bool) -> Result<(), String> {
        self.gated.lock().unwrap().inner.start(sink, input)
    }

    fn set_input(&mut self, input: bool) {
        self.gated.lock().unwrap().inner.set_input(input);
    }

    fn play(&mut self, utterance: &str, chunk: usize, samples: Vec<f32>, sample_rate: u32) {
        let mut gated = self.gated.lock().unwrap();
        if gated.paused {
            gated.held.push(Chunk { utterance: utterance.to_owned(), chunk, samples, sample_rate });
        } else {
            gated.inner.play(utterance, chunk, samples, sample_rate);
        }
    }

    fn stop_playback(&mut self) {
        let mut gated = self.gated.lock().unwrap();
        gated.held.clear();
        gated.inner.stop_playback();
    }

    fn stop(&mut self) {
        let mut gated = self.gated.lock().unwrap();
        gated.held.clear();
        gated.inner.stop();
    }
}
