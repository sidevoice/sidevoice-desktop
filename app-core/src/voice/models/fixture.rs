//! The recorded speech a call can be run on without a microphone: a stand-in for the call's audio that says a clip of
//! LibriSpeech (test/fixtures/voice) and plays at once what it is given. The real-model tests use it, and so does the
//! memory measurement the app runs (`probe`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sidevoice_voice::{AudioIo, IoEvent, IoSink};

/// The LibriSpeech clip (test/fixtures/voice), as samples from -1 to 1.
pub fn speech() -> Vec<f32> {
    let wav: &[u8] = include_bytes!("../../../../test/fixtures/voice/librispeech_mr_quilter.wav");
    let mut at = 12;
    while &wav[at..at + 4] != b"data" {
        at += 8 + u32::from_le_bytes(wav[at + 4..at + 8].try_into().unwrap()) as usize;
    }
    wav[at + 8..].as_chunks::<2>().0.iter().map(|pair| f32::from(i16::from_le_bytes(*pair)) / 32_768.0).collect()
}

/// A microphone that says the clip, then silence, in real time, and a speaker that plays at once what it is given.
pub struct Fixture {
    pub sink: Option<IoSink>,
    pub stop: Arc<AtomicBool>,
    pub played: Arc<Mutex<Vec<usize>>>,
}

impl AudioIo for Fixture {
    fn start(&mut self, sink: IoSink, input: bool) -> Result<(), String> {
        assert!(input, "the call listens, so it asks for the microphone");
        self.sink = Some(sink.clone());
        let stop = Arc::clone(&self.stop);
        std::thread::spawn(move || {
            sink.send(IoEvent::Ready);
            let heard = speech().into_iter().chain(std::iter::repeat_n(0.0, 16_000 * 4)).collect::<Vec<_>>();
            for frame in heard.chunks(160) {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                sink.send(IoEvent::Captured(frame.to_vec()));
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Ok(())
    }

    fn set_input(&mut self, _input: bool) {}

    fn play(&mut self, utterance: &str, chunk: usize, samples: Vec<f32>, _sample_rate: u32) {
        self.played.lock().unwrap().push(samples.len());
        if let Some(sink) = &self.sink {
            sink.send(IoEvent::ChunkStarted { utterance: utterance.into(), chunk });
            sink.send(IoEvent::ChunkPlayed { utterance: utterance.into(), chunk });
        }
    }

    fn stop_playback(&mut self) {}

    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}
