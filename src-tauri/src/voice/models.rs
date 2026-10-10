//! The voice call's models on the app's engine: sidevoice-voice's interfaces (`VoiceModels` and its slots) filled
//! with the models core's `voice::choose` picks, each loaded by its catalogue (`engine.catalog(id).load(model)`: the
//! catalogue picks the build, and a local one is installed first if it is not) and called through what it can do
//! (`as_vad`, `as_stt`, `as_tts`, `as_end_of_turn`). Failures are the engine's codes.
//!
//! Nothing native runs on an async worker of the app's runtime. Loading each model and opening the detector's stream
//! (reading model files, building ONNX sessions, initialising Metal), transcribing, speaking and ending turns run on
//! a blocking thread each. The detector runs one 32 ms window at a time where it is called, inside `block_in_place`,
//! so the worker hands its other tasks on for that while.

#[cfg(test)]
mod tests;

use std::future::Future;
use std::sync::Arc;

use sidevoice_desktop_core::voice::{ModelChoice, VoiceChoice};
use sidevoice_desktop_engine::sidevoice_engine::{Cancel, Engine, Error, Model, Progress, VadOptions, VadStream};
use sidevoice_voice::{async_trait, EndOfTurnModel, Models, Speaker, Transcriber, Vad, VadFrame, VoiceModels};

/// The rate sidevoice-voice hands the detector.
const VAD_RATE: u32 = 16_000;

/// How the detector decides: a window is speech from a probability of 0.6, speech counts from 400 ms of it, and ends
/// after 200 ms without.
const VAD_OPTIONS: VadOptions = VadOptions { threshold: 0.6, min_silence_ms: 200, min_speech_ms: 400 };

/// The models of `choice`, on `engine`.
pub struct EngineModels {
    engine: Arc<Engine>,
    choice: VoiceChoice,
}

impl EngineModels {
    pub fn new(engine: Arc<Engine>, choice: VoiceChoice) -> Self {
        Self { engine, choice }
    }

    async fn load_one(&self, chosen: &ModelChoice) -> Result<Arc<dyn Model>, String> {
        let (engine, catalog, model) = (Arc::clone(&self.engine), chosen.catalog.clone(), chosen.model.clone());
        blocking(async move {
            let progress = |_: Progress| {};
            let loaded = engine.catalog(&catalog)?.load(&model, &progress, &Cancel::new()).await?;
            Ok(Arc::from(loaded))
        })
        .await
    }
}

#[async_trait]
impl VoiceModels for EngineModels {
    async fn load(&self) -> Result<Models, String> {
        let detector = self.load_one(&self.choice.vad).await?;
        let vad = blocking(async move {
            let vad = detector.as_vad().ok_or(Error::new("model-cannot-detect"))?;
            vad.stream(VAD_OPTIONS).await
        })
        .await?;
        if vad.sample_rate() != VAD_RATE {
            return Err("vad-sample-rate".into());
        }
        let stt = self.load_one(&self.choice.stt).await?;
        let tts = self.load_one(&self.choice.tts).await?;
        let mut models = Models::new(Box::new(EngineVad(vad)), Arc::new(EngineStt(stt)), Arc::new(EngineTts(tts)));
        if let Some(chosen) = &self.choice.end_of_turn {
            models.end_of_turn = Some(Arc::new(EngineEndOfTurn(self.load_one(chosen).await?)));
        }
        Ok(models)
    }
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
