use std::sync::{Arc, Mutex};

use sidevoice_voice::{AudioIo, IoSink};

use super::PlaybackPause;

/// A speaker that records what it is given.
#[derive(Clone, Default)]
struct Speaker {
    log: Arc<Mutex<Vec<String>>>,
}

impl AudioIo for Speaker {
    fn start(&mut self, _sink: IoSink, _input: bool) -> Result<(), String> {
        Ok(())
    }

    fn set_input(&mut self, _input: bool) {}

    fn play(&mut self, utterance: &str, chunk: usize, _samples: Vec<f32>, _sample_rate: u32) {
        self.log.lock().unwrap().push(format!("{utterance}#{chunk}"));
    }

    fn stop_playback(&mut self) {
        self.log.lock().unwrap().push("stop_playback".into());
    }

    fn stop(&mut self) {
        self.log.lock().unwrap().push("stop".into());
    }
}

fn lane(pause: &PlaybackPause) -> (Box<dyn AudioIo>, Arc<Mutex<Vec<String>>>) {
    let speaker = Speaker::default();
    let log = Arc::clone(&speaker.log);
    (pause.wrap(Box::new(speaker)), log)
}

fn heard(log: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    log.lock().unwrap().clone()
}

#[test]
fn an_unpaused_lane_plays_at_once() {
    let pause = PlaybackPause::default();
    let (mut io, log) = lane(&pause);
    io.play("a", 0, vec![0.0], 16_000);
    assert_eq!(heard(&log), ["a#0"]);
}

#[test]
fn a_pause_holds_the_chunks_and_resuming_plays_them_in_order() {
    let pause = PlaybackPause::default();
    let (mut io, log) = lane(&pause);
    io.play("a", 0, vec![0.0], 16_000);
    pause.set(true);
    io.play("a", 1, vec![0.0], 16_000);
    io.play("a", 2, vec![0.0], 16_000);
    assert_eq!(heard(&log), ["a#0"]);
    pause.set(true); // again: nothing
    pause.set(false);
    assert_eq!(heard(&log), ["a#0", "a#1", "a#2"]);
    io.play("a", 3, vec![0.0], 16_000);
    assert_eq!(heard(&log), ["a#0", "a#1", "a#2", "a#3"]);
}

#[test]
fn a_lane_made_during_a_pause_is_paused() {
    let pause = PlaybackPause::default();
    pause.set(true);
    let (mut io, log) = lane(&pause);
    io.play("a", 0, vec![0.0], 16_000);
    assert!(heard(&log).is_empty());
    pause.set(false);
    assert_eq!(heard(&log), ["a#0"]);
}

#[test]
fn a_cut_off_or_stopped_call_drops_what_was_held() {
    let pause = PlaybackPause::default();
    let (mut io, log) = lane(&pause);
    pause.set(true);
    io.play("a", 0, vec![0.0], 16_000);
    io.stop_playback();
    pause.set(false);
    assert_eq!(heard(&log), ["stop_playback"]);

    pause.set(true);
    io.play("b", 0, vec![0.0], 16_000);
    io.stop();
    pause.set(false);
    assert_eq!(heard(&log), ["stop_playback", "stop"]);
}

#[test]
fn a_lane_that_is_gone_is_forgotten() {
    let pause = PlaybackPause::default();
    let (io, _log) = lane(&pause);
    drop(io);
    pause.set(true);
    pause.set(false);
    assert!(pause.state.lock().unwrap().lanes.is_empty());
}
