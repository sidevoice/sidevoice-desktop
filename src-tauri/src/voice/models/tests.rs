//! The halves load only the slots they use, keep their models while their slots stay, and tell a slot that cannot load
//! by its code, with fakes for the engine.

use std::sync::Mutex;

use serde_json::json;
use sidevoice_desktop_core::voice::ModelChoice;
use sidevoice_voice::{EndOfTurnModel, Speaker, Transcriber, Vad, VadFrame};

use super::*;

/// The 10 ms frames of tone that make a turn: 1.5 s. sidevoice-voice hears speech start after 400 ms of it at the
/// volume bar, the volume measured over the last 400 ms and smoothed (Pipecat's, sidevoice-voice#25): 800 ms of tone no
/// longer opens a turn, a second does.
pub(crate) const TURN_FRAMES: usize = 150;

/// A detector that hears speech in any 32 ms window louder than a whisper.
#[derive(Default)]
struct LoudVad {
    pending: Vec<f32>,
    taken: u64,
}

#[async_trait]
impl Vad for LoudVad {
    async fn accept(&mut self, pcm: &[f32]) -> Result<Vec<VadFrame>, String> {
        self.pending.extend_from_slice(pcm);
        let mut frames = Vec::new();
        while self.pending.len() >= 512 {
            let window: Vec<f32> = self.pending.drain(..512).collect();
            self.taken += 512;
            let loud = window.iter().map(|s| s * s).sum::<f32>() / 512.0 > 0.001;
            frames.push(VadFrame { end: self.taken, speech: loud, probability: None });
        }
        Ok(frames)
    }

    async fn reset(&mut self) {
        self.pending.clear();
        self.taken = 0;
    }
}

/// A model that answers `name`.
pub(crate) struct Named {
    name: &'static str,
}

#[async_trait]
impl Transcriber for Named {
    async fn transcribe(&self, _pcm: Vec<f32>, _rate: u32, _language: Option<String>) -> Result<String, String> {
        Ok(self.name.to_string())
    }
}

#[async_trait]
impl Speaker for Named {
    async fn speak(
        &self,
        _text: String,
        _voice: Option<String>,
        _language: Option<String>,
        _speed: f32,
    ) -> Result<(Vec<f32>, u32), String> {
        Ok((vec![0.1; 4_800], 24_000))
    }
}

#[async_trait]
impl EndOfTurnModel for Named {
    async fn end_of_turn(&self, _pcm: Vec<f32>, _rate: u32) -> Result<f32, String> {
        Ok(1.0)
    }
}

/// The engine as a test has it: models by id that every load hands out (the same one each time, as the engine's cache
/// does), `fail` one that cannot be loaded, and a record of what was loaded.
pub(crate) struct FakeLoader {
    models: Vec<Arc<Named>>,
    /// Every load, as `slot:model` (`vad` for the detector).
    pub(crate) loads: Mutex<Vec<String>>,
    /// Shut, a load of the transcriber `slow` waits; open, it goes on.
    gate: tokio::sync::watch::Sender<bool>,
}

impl FakeLoader {
    pub(crate) fn new(names: &[&'static str]) -> Arc<Self> {
        Arc::new(FakeLoader {
            models: names.iter().map(|name| Arc::new(Named { name })).collect(),
            loads: Mutex::new(Vec::new()),
            gate: tokio::sync::watch::channel(false).0,
        })
    }

    fn find(&self, slot: &str, chosen: &ModelChoice) -> Result<Arc<Named>, String> {
        self.loads.lock().unwrap().push(format!("{slot}:{}", chosen.model));
        if chosen.model == "fail" {
            return Err("model-load-failed".into());
        }
        Ok(Arc::clone(self.models.iter().find(|m| m.name == chosen.model).expect("a model of the test")))
    }

    /// Lets the loads of `slow` go on.
    pub(crate) fn open_gate(&self) {
        self.gate.send_replace(true);
    }

    /// How many times `slot` was loaded.
    pub(crate) fn loaded(&self, slot: &str) -> usize {
        self.loads.lock().unwrap().iter().filter(|load| load.starts_with(&format!("{slot}:"))).count()
    }

    /// How many holders model `name` has besides the loader's own.
    pub(crate) fn holders(&self, name: &str) -> usize {
        Arc::strong_count(self.models.iter().find(|m| m.name == name).unwrap()) - 1
    }
}

#[async_trait]
impl Loader for Arc<FakeLoader> {
    async fn vad(&self, chosen: &ModelChoice) -> Result<Box<dyn Vad>, String> {
        self.loads.lock().unwrap().push(format!("vad:{}", chosen.model));
        Ok(Box::new(LoudVad::default()))
    }
    async fn transcriber(&self, chosen: &ModelChoice) -> Result<Arc<dyn Transcriber>, String> {
        if chosen.model == "slow" {
            let _ = self.gate.subscribe().wait_for(|open| *open).await;
        }
        Ok(self.find("stt", chosen)?)
    }
    async fn speaker(&self, chosen: &ModelChoice) -> Result<Arc<dyn Speaker>, String> {
        Ok(self.find("tts", chosen)?)
    }
    async fn end_of_turn(&self, chosen: &ModelChoice) -> Result<Arc<dyn EndOfTurnModel>, String> {
        Ok(self.find("eot", chosen)?)
    }
}

/// The loader as the halves take it.
pub(crate) fn loader(fake: &Arc<FakeLoader>) -> Arc<dyn Loader> {
    Arc::new(Arc::clone(fake))
}

/// The choice of a call: an input half when `stt` is given (with `smart`'s model, else silence), an output half when
/// `tts` is.
pub(crate) fn chosen(stt: Option<&str>, tts: Option<&str>, smart: Option<&str>) -> VoiceChoice {
    VoiceChoice {
        input: stt.map(|stt| InputChoice {
            vad: ModelChoice::local("vad"),
            stt: ModelChoice::local(stt),
            end_of_turn: smart.map(ModelChoice::local),
            config: json!({
                "language": "en", "patience": "fast",
                "end_of_turn": if smart.is_some() { json!({"kind": "smart"}) } else { json!({"kind": "silence"}) },
            }),
        }),
        output: tts.map(|tts| OutputChoice { tts: ModelChoice::local(tts), config: json!({"voice": tts}) }),
        config: json!({"audio_grace_ms": 0}),
    }
}

#[test]
fn the_slots_a_half_uses_are_the_ones_it_loads() {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let fake = FakeLoader::new(&["words", "voice", "ends"]);
        let loader = loader(&fake);

        // A transcription try: the detector and the transcriber, no speaker, no end-of-turn model.
        let (loaded, failed) = Halves::new(&loader, &chosen(Some("words"), None, None), None).load().await;
        assert_eq!(failed, None);
        let mut loads = fake.loads.lock().unwrap().clone();
        loads.sort();
        assert_eq!(loads, ["stt:words", "vad:vad"]);
        drop(loaded);

        // A voice try: the speaker alone.
        fake.loads.lock().unwrap().clear();
        let (_loaded, failed) = Halves::new(&loader, &chosen(None, Some("voice"), None), None).load().await;
        assert_eq!(failed, None);
        assert_eq!(fake.loads.lock().unwrap().clone(), ["tts:voice"]);

        // A call: both halves, and the model that ends turns for `smart`.
        fake.loads.lock().unwrap().clear();
        let (_loaded, failed) =
            Halves::new(&loader, &chosen(Some("words"), Some("voice"), Some("ends")), None).load().await;
        let mut loads = fake.loads.lock().unwrap().clone();
        loads.sort();
        assert_eq!(
            (failed, loads),
            (None, ["eot:ends", "stt:words", "tts:voice", "vad:vad"].map(String::from).to_vec())
        );
    });
}

#[test]
fn a_slot_that_cannot_load_is_told_by_its_code_and_the_others_stay_loaded() {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let fake = FakeLoader::new(&["words", "fail"]);
        let (loaded, failed) =
            Halves::new(&loader(&fake), &chosen(Some("words"), Some("fail"), None), None).load().await;
        assert_eq!(failed.as_deref(), Some("model-load-failed"));
        assert_eq!(fake.holders("words"), 1, "the transcriber that loaded is held");
        drop(loaded);
        assert_eq!(fake.holders("words"), 0);
    });
}

#[test]
fn a_half_with_the_same_slots_keeps_its_models_and_one_with_other_slots_gets_its_own() {
    let fake = FakeLoader::new(&["words", "other", "voice"]);
    let loader = loader(&fake);
    let first = Halves::new(&loader, &chosen(Some("words"), Some("voice"), None), None);
    let (first_in, first_out) = (first.input.as_ref().unwrap().1.clone(), first.output.as_ref().unwrap().1.clone());

    // The same slots, another configuration: the same models, so an update keeps what they loaded.
    let mut again = chosen(Some("words"), Some("voice"), None);
    again.input.as_mut().unwrap().config = json!({"language": "es", "end_of_turn": {"kind": "silence", "ms": 900}});
    again.output.as_mut().unwrap().config = json!({"voice": "af_heart", "speed": 0.8});
    let kept = Halves::new(&loader, &again, Some(&first));
    assert!(Arc::ptr_eq(&first_in, &kept.input.as_ref().unwrap().1));
    assert!(Arc::ptr_eq(&first_out, &kept.output.as_ref().unwrap().1));
    // And the setup carries the new configuration.
    let setup = kept.setup().unwrap();
    assert_eq!(setup.input.unwrap().config.language.as_deref(), Some("es"));
    assert_eq!(setup.output.unwrap().config.voice.as_deref(), Some("af_heart"));

    // Another transcriber: the input half is new, the output half is the same.
    let moved = Halves::new(&loader, &chosen(Some("other"), Some("voice"), None), Some(&first));
    assert!(!Arc::ptr_eq(&first_in, &moved.input.as_ref().unwrap().1));
    assert!(Arc::ptr_eq(&first_out, &moved.output.as_ref().unwrap().1));

    // A half added, a half removed.
    let added = Halves::new(
        &loader,
        &chosen(Some("words"), Some("voice"), None),
        Some(&Halves::new(&loader, &chosen(None, Some("voice"), None), None)),
    );
    assert!(added.input.is_some());
    let removed = Halves::new(&loader, &chosen(None, Some("voice"), None), Some(&first));
    assert!(removed.input.is_none() && removed.setup().unwrap().input.is_none());
}
