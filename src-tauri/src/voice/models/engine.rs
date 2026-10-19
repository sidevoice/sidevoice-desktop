//! The engine behind the call's model slots: sidevoice-engine's models, each loaded by its catalogue (or as its build for
//! a local model with a build chosen), shared by the calls and preloads that hold them, and called through what they can
//! do (`as_vad`, `as_stt`, `as_tts`, `as_end_of_turn`).
//!
//! Nothing native runs on an async worker of the app's runtime. Loading each model and opening the detector's stream
//! (reading model files, building ONNX sessions, initialising Metal), transcribing, speaking and ending turns run on
//! a blocking thread each. The detector runs one 32 ms window at a time where it is called, inside `block_in_place`,
//! so the worker hands its other tasks on for that while.

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use sidevoice_desktop_core::voice::{ModelChoice, LOCAL_CATALOG};
use sidevoice_desktop_engine::sidevoice_engine::{Cancel, Engine, Error, Model, Progress, VadOptions, VadStream};
use sidevoice_voice::{async_trait, EndOfTurnModel, Speaker, Transcriber, Vad, VadFrame};

use super::Loader;

/// The rate sidevoice-voice hands the detector.
const VAD_RATE: u32 = 16_000;

/// Native Silero supplies no per-window probability (sidevoice-engine#99), so sidevoice-voice uses its speech flag.
/// Minimize sherpa's holds to avoid stacking them with the Pipecat-style turn chain while that issue is unresolved.
const VAD_OPTIONS: VadOptions = VadOptions { threshold: 0.6, min_silence_ms: 1, min_speech_ms: 1 };

/// One cache for the desktop host. It holds weak references so models nobody holds are let go; calls asking for the
/// same model share the load and the resulting instance, and loads of different models go on at the same time.
#[derive(Default)]
pub struct ModelCache {
    loaded: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Option<Weak<dyn Model>>>>>>,
}

/// The engine's models, each loaded by its catalogue, or as its build for a local model with a build chosen.
pub(crate) struct EngineLoader {
    engine: Arc<Engine>,
    cache: Arc<ModelCache>,
}

impl EngineLoader {
    pub(crate) fn new(engine: Arc<Engine>, cache: Arc<ModelCache>) -> Arc<dyn Loader> {
        Arc::new(EngineLoader { engine, cache })
    }

    async fn load(&self, chosen: &ModelChoice) -> Result<Arc<dyn Model>, String> {
        let key = format!("{}:{}:{}", chosen.catalog, chosen.model, chosen.build.as_deref().unwrap_or(""));
        let entry = Arc::clone(lock(&self.cache.loaded).entry(key).or_default());
        let mut held = entry.lock().await;
        if let Some(model) = held.as_ref().and_then(Weak::upgrade) {
            return Ok(model);
        }
        let (engine, chosen) = (Arc::clone(&self.engine), chosen.clone());
        let model = blocking(async move {
            let progress = |_: Progress| {};
            let loaded: Arc<dyn Model> = match chosen.build.as_deref() {
                Some(build) if chosen.catalog == LOCAL_CATALOG => {
                    Arc::new(engine.load(&chosen.model, Some(build), &progress, &Cancel::new()).await?)
                }
                _ => Arc::from(engine.catalog(&chosen.catalog)?.load(&chosen.model, &progress, &Cancel::new()).await?),
            };
            Ok(loaded)
        })
        .await?;
        *held = Some(Arc::downgrade(&model));
        Ok(model)
    }
}

#[async_trait]
impl Loader for EngineLoader {
    async fn vad(&self, chosen: &ModelChoice) -> Result<Box<dyn Vad>, String> {
        let detector = self.load(chosen).await?;
        let model = Arc::clone(&detector);
        let vad = blocking(async move {
            let vad = detector.as_vad().ok_or(Error::new("model-cannot-detect"))?;
            vad.stream(VAD_OPTIONS).await
        })
        .await?;
        if vad.sample_rate() != VAD_RATE {
            return Err("vad-sample-rate".into());
        }
        Ok(Box::new(EngineVad { stream: vad, _model: model }))
    }

    async fn transcriber(&self, chosen: &ModelChoice) -> Result<Arc<dyn Transcriber>, String> {
        Ok(Arc::new(EngineStt(self.load(chosen).await?)))
    }

    async fn speaker(&self, chosen: &ModelChoice) -> Result<Arc<dyn Speaker>, String> {
        Ok(Arc::new(EngineTts(self.load(chosen).await?)))
    }

    async fn end_of_turn(&self, chosen: &ModelChoice) -> Result<Arc<dyn EndOfTurnModel>, String> {
        Ok(Arc::new(EngineEndOfTurn(self.load(chosen).await?)))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A detector stream, which keeps the detector model it came from loaded for as long as the stream is held.
struct EngineVad {
    stream: VadStream,
    _model: Arc<dyn Model>,
}

#[async_trait]
impl Vad for EngineVad {
    async fn accept(&mut self, pcm: &[f32]) -> Result<Vec<VadFrame>, String> {
        let stream = &mut self.stream;
        let output = tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(stream.accept(pcm)))
            .map_err(code)?;
        Ok(output
            .frames
            .into_iter()
            .map(|frame| VadFrame { end: frame.end, speech: frame.speech, probability: frame.probability })
            .collect())
    }

    async fn reset(&mut self) {
        self.stream.reset();
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
