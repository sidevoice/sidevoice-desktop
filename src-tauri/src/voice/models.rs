//! The voice call's models on the app's engine: sidevoice-voice's interfaces (`VoiceModels` and its slots) filled
//! with the models core's `voice::choose` picks, each loaded by its catalogue (`engine.catalog(id).load(model)`: the
//! catalogue picks the build), or, for a local model with a build chosen, as that build (`engine.load(model, build)`),
//! a local one installed first if it is not, and called through what it can do
//! (`as_vad`, `as_stt`, `as_tts`, `as_end_of_turn`). Failures are the engine's codes.
//!
//! A call keeps its models while the person changes them (sidevoice-web #75, gap #31): the call holds one stable
//! interface per slot ([`Slots`]), and each calls whichever model the slot holds now. Changing a model
//! ([`EngineModels::switch`]) loads the new one while the old one goes on serving, then moves the slot to it; work
//! already under way finishes on the model it started with, which it holds. The call is never stopped for it. The voice
//! activity detector is the app's own choice and a stream with a position, so it stays: a different one is taken at the
//! call's next load. Text to speech keeps the voice chosen with its model, so a reply never asks a model for another's
//! voice.
//!
//! Nothing native runs on an async worker of the app's runtime. Loading each model and opening the detector's stream
//! (reading model files, building ONNX sessions, initialising Metal), transcribing, speaking and ending turns run on
//! a blocking thread each. The detector runs one 32 ms window at a time where it is called, inside `block_in_place`,
//! so the worker hands its other tasks on for that while.

#[cfg(test)]
mod tests;

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use sidevoice_desktop_core::voice::{ModelChoice, VoiceChoice, LOCAL_CATALOG};
use sidevoice_desktop_engine::sidevoice_engine::{Cancel, Engine, Error, Model, Progress, VadOptions, VadStream};
use sidevoice_voice::{async_trait, EndOfTurnModel, Models, Speaker, Transcriber, Vad, VadFrame, VoiceModels};

/// The rate sidevoice-voice hands the detector.
const VAD_RATE: u32 = 16_000;

/// How the detector decides: a window is speech from a probability of 0.6, speech counts from 400 ms of it, and ends
/// after 200 ms without.
const VAD_OPTIONS: VadOptions = VadOptions { threshold: 0.6, min_silence_ms: 200, min_speech_ms: 400 };

/// The models of the choice made last, on `engine`: loaded into fresh [`Slots`] when the call loads, and switched in
/// those slots while the call has them.
pub struct EngineModels {
    engine: Arc<Engine>,
    choice: Mutex<VoiceChoice>,
    /// The slots of the call's loaded models, while it keeps them (it drops them when idle).
    live: Mutex<Weak<Slots>>,
    /// One load or switch at a time: a switch never lands in the middle of a load, nor two switches out of order.
    turn: tokio::sync::Mutex<()>,
    /// The switches asked for so far: one that a later one overtook while it waited or loaded changes nothing.
    changes: AtomicU64,
}

/// A switch that took place: the slots hold its models, and no other switch moves them while this is held, so the
/// configuration that goes with them is applied before any later one.
pub struct Switched<'a> {
    _turn: tokio::sync::MutexGuard<'a, ()>,
}

impl EngineModels {
    pub fn new(engine: Arc<Engine>, choice: VoiceChoice) -> Self {
        Self {
            engine,
            choice: Mutex::new(choice),
            live: Mutex::default(),
            turn: tokio::sync::Mutex::new(()),
            changes: AtomicU64::new(0),
        }
    }

    /// Makes `choice` the call's: a slot whose model changes gets the new one once it is loaded (the old one serving
    /// until then, and finishing what it started), and text to speech its voice at once. Without loaded slots (no call
    /// yet, or one that dropped its models), the choice is what the next load takes. The latest change wins: one that a
    /// later one overtook (while it waited, or while its models loaded) changes nothing and answers `None`. A model
    /// that cannot be loaded leaves the slots as they were, and the choice too: its code. `Some` holds off any later
    /// switch until it is dropped, for the configuration that goes with these models to apply first.
    pub async fn switch(&self, choice: VoiceChoice) -> Result<Option<Switched<'_>>, String> {
        let change = self.changes.fetch_add(1, Ordering::SeqCst) + 1;
        let turn = self.turn.lock().await;
        let latest = || self.changes.load(Ordering::SeqCst) == change;
        if !latest() {
            return Ok(None);
        }
        let current = lock(&self.choice).clone();
        let live = lock(&self.live).upgrade();
        let Some(slots) = live.filter(|_| !current.same_models(&choice)) else {
            if let Some(slots) = lock(&self.live).upgrade() {
                slots.set_voice(voice_of(&choice));
            }
            *lock(&self.choice) = choice;
            return Ok(Some(Switched { _turn: turn }));
        };
        // Everything new is loaded first, so a failure changes nothing.
        let loaded = async {
            let stt = if current.stt == choice.stt { None } else { Some(EngineStt(self.load_one(&choice.stt).await?)) };
            let tts = if current.tts == choice.tts { None } else { Some(EngineTts(self.load_one(&choice.tts).await?)) };
            let end_of_turn = match (current.end_of_turn == choice.end_of_turn, &choice.end_of_turn) {
                (true, _) => None,
                (false, Some(chosen)) => Some(Some(EngineEndOfTurn(self.load_one(chosen).await?))),
                (false, None) => Some(None),
            };
            Ok::<_, String>((stt, tts, end_of_turn))
        }
        .await;
        if !latest() {
            // Overtaken while loading: what loaded (or failed to) is dropped, and the later change applies instead.
            return Ok(None);
        }
        let (stt, tts, end_of_turn) = loaded?;
        if let Some(stt) = stt {
            slots.set_transcriber(Arc::new(stt));
        }
        let voice = voice_of(&choice);
        match tts {
            Some(tts) => slots.set_speaker(Arc::new(tts), voice),
            None => slots.set_voice(voice),
        }
        if let Some(end_of_turn) = end_of_turn {
            slots.set_end_of_turn(end_of_turn.map(|model| Arc::new(model) as Arc<dyn EndOfTurnModel>));
        }
        *lock(&self.choice) = choice;
        Ok(Some(Switched { _turn: turn }))
    }

    async fn load_one(&self, chosen: &ModelChoice) -> Result<Arc<dyn Model>, String> {
        let (engine, chosen) = (Arc::clone(&self.engine), chosen.clone());
        blocking(async move {
            let progress = |_: Progress| {};
            let loaded: Arc<dyn Model> = match chosen.build.as_deref() {
                Some(build) if chosen.catalog == LOCAL_CATALOG => {
                    Arc::new(engine.load(&chosen.model, Some(build), &progress, &Cancel::new()).await?)
                }
                _ => Arc::from(engine.catalog(&chosen.catalog)?.load(&chosen.model, &progress, &Cancel::new()).await?),
            };
            Ok(loaded)
        })
        .await
    }
}

#[async_trait]
impl VoiceModels for EngineModels {
    async fn load(&self) -> Result<Models, String> {
        let _turn = self.turn.lock().await;
        let choice = lock(&self.choice).clone();
        let detector = self.load_one(&choice.vad).await?;
        let vad = blocking(async move {
            let vad = detector.as_vad().ok_or(Error::new("model-cannot-detect"))?;
            vad.stream(VAD_OPTIONS).await
        })
        .await?;
        if vad.sample_rate() != VAD_RATE {
            return Err("vad-sample-rate".into());
        }
        let stt = self.load_one(&choice.stt).await?;
        let tts = self.load_one(&choice.tts).await?;
        let end_of_turn = match &choice.end_of_turn {
            Some(chosen) => Some(Arc::new(EngineEndOfTurn(self.load_one(chosen).await?)) as Arc<dyn EndOfTurnModel>),
            None => None,
        };
        let slots = Slots::new(Arc::new(EngineStt(stt)), Arc::new(EngineTts(tts)), voice_of(&choice), end_of_turn);
        *lock(&self.live) = Arc::downgrade(&slots);
        Ok(slots.models(Box::new(EngineVad(vad))))
    }
}

/// The voice chosen with the text-to-speech model, from the configuration it came with (`None`: the model's first).
fn voice_of(choice: &VoiceChoice) -> Option<String> {
    choice.config.get("voice").and_then(|voice| voice.as_str()).map(str::to_string)
}

/// What fills each slot of a call now. The call holds these through [`Slots::models`]; each call to a slot takes the
/// model the slot holds as it starts, and keeps it to the end.
pub(crate) struct Slots {
    transcriber: Mutex<Arc<dyn Transcriber>>,
    /// The speaker, and the voice chosen with it.
    speaker: Mutex<(Arc<dyn Speaker>, Option<String>)>,
    end_of_turn: Mutex<Option<Arc<dyn EndOfTurnModel>>>,
}

impl Slots {
    pub(crate) fn new(
        transcriber: Arc<dyn Transcriber>,
        speaker: Arc<dyn Speaker>,
        voice: Option<String>,
        end_of_turn: Option<Arc<dyn EndOfTurnModel>>,
    ) -> Arc<Self> {
        Arc::new(Slots {
            transcriber: Mutex::new(transcriber),
            speaker: Mutex::new((speaker, voice)),
            end_of_turn: Mutex::new(end_of_turn),
        })
    }

    /// The call's models over these slots, with `vad`. The end of turn is always there, so that a call may turn to
    /// `smart-turn` without loading again; asked while its slot is empty, it says `end-of-turn-missing`.
    pub(crate) fn models(self: &Arc<Self>, vad: Box<dyn Vad>) -> Models {
        let mut models =
            Models::new(vad, Arc::new(SlotTranscriber(Arc::clone(self))), Arc::new(SlotSpeaker(Arc::clone(self))));
        models.end_of_turn = Some(Arc::new(SlotEndOfTurn(Arc::clone(self))));
        models
    }

    pub(crate) fn set_transcriber(&self, transcriber: Arc<dyn Transcriber>) {
        *lock(&self.transcriber) = transcriber;
    }

    pub(crate) fn set_speaker(&self, speaker: Arc<dyn Speaker>, voice: Option<String>) {
        *lock(&self.speaker) = (speaker, voice);
    }

    pub(crate) fn set_voice(&self, voice: Option<String>) {
        lock(&self.speaker).1 = voice;
    }

    pub(crate) fn set_end_of_turn(&self, end_of_turn: Option<Arc<dyn EndOfTurnModel>>) {
        *lock(&self.end_of_turn) = end_of_turn;
    }
}

struct SlotTranscriber(Arc<Slots>);

#[async_trait]
impl Transcriber for SlotTranscriber {
    async fn transcribe(&self, pcm: Vec<f32>, sample_rate: u32, language: Option<String>) -> Result<String, String> {
        let model = Arc::clone(&*lock(&self.0.transcriber));
        model.transcribe(pcm, sample_rate, language).await
    }
}

struct SlotSpeaker(Arc<Slots>);

#[async_trait]
impl Speaker for SlotSpeaker {
    /// With the voice chosen with the model the slot holds (the call's own may name the model before).
    async fn speak(
        &self,
        text: String,
        _voice: Option<String>,
        language: Option<String>,
        speed: f32,
    ) -> Result<(Vec<f32>, u32), String> {
        let (model, voice) = lock(&self.0.speaker).clone();
        model.speak(text, voice, language, speed).await
    }
}

struct SlotEndOfTurn(Arc<Slots>);

#[async_trait]
impl EndOfTurnModel for SlotEndOfTurn {
    async fn end_of_turn(&self, pcm: Vec<f32>, sample_rate: u32) -> Result<f32, String> {
        let model = lock(&self.0.end_of_turn).clone().ok_or_else(|| "end-of-turn-missing".to_string())?;
        model.end_of_turn(pcm, sample_rate).await
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct EngineVad(VadStream);

#[async_trait]
impl Vad for EngineVad {
    async fn accept(&mut self, pcm: &[f32]) -> Result<Vec<VadFrame>, String> {
        let stream = &mut self.0;
        let output = tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(stream.accept(pcm)))
            .map_err(code)?;
        Ok(output
            .frames
            .into_iter()
            .map(|frame| VadFrame { end: frame.end, speech: frame.speech, probability: frame.probability })
            .collect())
    }

    async fn reset(&mut self) {
        self.0.reset();
    }
}

struct EngineStt(Arc<dyn Model>);

#[async_trait]
impl Transcriber for EngineStt {
    async fn transcribe(&self, pcm: Vec<f32>, sample_rate: u32, language: Option<String>) -> Result<String, String> {
        let model = Arc::clone(&self.0);
        blocking(async move {
            let stt = model.as_stt().ok_or(Error::new("model-cannot-transcribe"))?;
            stt.transcribe(&pcm, sample_rate, language.as_deref()).await
        })
        .await
    }
}

struct EngineTts(Arc<dyn Model>);

#[async_trait]
impl Speaker for EngineTts {
    /// With no `voice`, the model's first.
    async fn speak(
        &self,
        text: String,
        voice: Option<String>,
        language: Option<String>,
        speed: f32,
    ) -> Result<(Vec<f32>, u32), String> {
        let model = Arc::clone(&self.0);
        blocking(async move {
            let tts = model.as_tts().ok_or(Error::new("model-cannot-speak"))?;
            let voice = match voice {
                Some(voice) => voice,
                None => tts.voices().await.into_iter().next().ok_or(Error::new("unknown-voice"))?.id,
            };
            let audio = tts.speak(&text, &voice, language.as_deref(), Some(speed)).await?;
            Ok((audio.samples, audio.sample_rate))
        })
        .await
    }
}

struct EngineEndOfTurn(Arc<dyn Model>);

#[async_trait]
impl EndOfTurnModel for EngineEndOfTurn {
    async fn end_of_turn(&self, pcm: Vec<f32>, sample_rate: u32) -> Result<f32, String> {
        let model = Arc::clone(&self.0);
        blocking(async move {
            let classifier = model.as_end_of_turn().ok_or(Error::new("model-cannot-end-turns"))?;
            classifier.probability(&pcm, sample_rate).await
        })
        .await
    }
}

/// Runs `work` to its end on a blocking thread of the runtime it is called on (the app's), off its async workers.
async fn blocking<T: Send + 'static>(
    work: impl Future<Output = Result<T, Error>> + Send + 'static,
) -> Result<T, String> {
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || runtime.block_on(work)).await.map_err(|_| "internal".to_string())?.map_err(code)
}

fn code(error: Error) -> String {
    error.code.to_string()
}
