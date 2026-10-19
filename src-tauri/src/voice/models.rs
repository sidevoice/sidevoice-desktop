//! The voice call's models on the app's engine: sidevoice-voice's interfaces (`InputModels` for the input half,
//! `OutputModels` for the output half, each slot asked for on its own) filled with the models core's `voice::choose`
//! picks, each loaded by its catalogue (`engine.catalog(id).load(model)`: the catalogue picks the build), or, for a
//! local model with a build chosen, as that build (`engine.load(model, build)`), a local one installed first if it is
//! not. Failures are the engine's codes.
//!
//! A call loads only the slots its halves use, and keeps what it loaded while it changes (sidevoice-voice's `update`:
//! a half whose models are the same `Arc` keeps them; [`Halves::new`] hands the same one back while a half's slots
//! stay). Calls share what is loaded: a model is loaded once while any call, or a preload, holds it ([`ModelCache`],
//! weak, per model), so a transcription try followed by a call loads the transcriber and the detector once.
//! The engine itself is `engine`'s.

#[cfg(test)]
pub(crate) mod tests;

mod engine;

use std::sync::Arc;

use sidevoice_desktop_core::voice::{InputChoice, ModelChoice, OutputChoice, VoiceChoice};
use sidevoice_voice::{
    async_trait, EndOfTurnModel, InputConfig, InputModels, OutputConfig, OutputModels, Speaker, Transcriber, Vad,
    VoiceConfig, VoiceInput, VoiceOutput, VoiceSetup,
};

pub(crate) use engine::{EngineLoader, ModelCache};

/// Loads each model the call's slots take: the engine's ([`EngineLoader`]), or a test's.
#[async_trait]
pub(crate) trait Loader: Send + Sync {
    async fn vad(&self, chosen: &ModelChoice) -> Result<Box<dyn Vad>, String>;
    async fn transcriber(&self, chosen: &ModelChoice) -> Result<Arc<dyn Transcriber>, String>;
    async fn speaker(&self, chosen: &ModelChoice) -> Result<Arc<dyn Speaker>, String>;
    async fn end_of_turn(&self, chosen: &ModelChoice) -> Result<Arc<dyn EndOfTurnModel>, String>;
}

/// The input half's models: the detector, the transcriber and, for `smart`, the end-of-turn model, each loaded when
/// the call asks for it.
pub(crate) struct EngineInput {
    loader: Arc<dyn Loader>,
    choice: InputChoice,
}

#[async_trait]
impl InputModels for EngineInput {
    async fn vad(&self) -> Result<Box<dyn Vad>, String> {
        self.loader.vad(&self.choice.vad).await
    }

    async fn transcriber(&self) -> Result<Arc<dyn Transcriber>, String> {
        self.loader.transcriber(&self.choice.stt).await
    }

    async fn end_of_turn(&self) -> Result<Arc<dyn EndOfTurnModel>, String> {
        match &self.choice.end_of_turn {
            Some(chosen) => self.loader.end_of_turn(chosen).await,
            None => Err("end-of-turn-missing".into()),
        }
    }
}

/// The output half's model: the speaker.
pub(crate) struct EngineOutput {
    loader: Arc<dyn Loader>,
    choice: ModelChoice,
}

#[async_trait]
impl OutputModels for EngineOutput {
    async fn speaker(&self) -> Result<Arc<dyn Speaker>, String> {
        self.loader.speaker(&self.choice).await
    }
}

/// The halves of a call as the app made them from the person's choices: each half's choice and its models. The same
/// models (one `Arc`) come back from [`Halves::new`] for a half whose slots did not change, so that
/// `VoiceCall::update` keeps what that half loaded.
#[derive(Clone)]
pub(crate) struct Halves {
    input: Option<(InputChoice, Arc<EngineInput>)>,
    output: Option<(OutputChoice, Arc<EngineOutput>)>,
    /// `VoiceConfig`: what holds for the whole call.
    config: serde_json::Value,
}

/// What a load holds: the models of every slot the halves use, kept for as long as this is.
#[derive(Default)]
pub(crate) struct Loaded {
    _vad: Option<Box<dyn Vad>>,
    _transcriber: Option<Arc<dyn Transcriber>>,
    _end_of_turn: Option<Arc<dyn EndOfTurnModel>>,
    _speaker: Option<Arc<dyn Speaker>>,
}

impl Halves {
    /// The halves of `choice`, over `loader`; a half that has the same models as `previous`' keeps them.
    pub(crate) fn new(loader: &Arc<dyn Loader>, choice: &VoiceChoice, previous: Option<&Halves>) -> Halves {
        let input = choice.input.as_ref().map(|input| {
            let same = previous.and_then(|p| p.input.as_ref()).filter(|(old, _)| old.same_models(input));
            let models = same.map_or_else(
                || Arc::new(EngineInput { loader: Arc::clone(loader), choice: input.clone() }),
                |(_, models)| Arc::clone(models),
            );
            (input.clone(), models)
        });
        let output = choice.output.as_ref().map(|output| {
            let same = previous.and_then(|p| p.output.as_ref()).filter(|(old, _)| old.same_models(output));
            let models = same.map_or_else(
                || Arc::new(EngineOutput { loader: Arc::clone(loader), choice: output.tts.clone() }),
                |(_, models)| Arc::clone(models),
            );
            (output.clone(), models)
        });
        Halves { input, output, config: choice.config.clone() }
    }

    /// What `VoiceCall::new` and `update` take: the halves with their configuration, and the call's own.
    pub(crate) fn setup(&self) -> Result<VoiceSetup, String> {
        let invalid = |what: &str, error: serde_json::Error| format!("invalid-{what}-config: {error}");
        let input = match &self.input {
            Some((choice, models)) => Some(VoiceInput {
                models: Arc::clone(models) as Arc<dyn InputModels>,
                config: serde_json::from_value::<InputConfig>(choice.config.clone())
                    .map_err(|e| invalid("input", e))?,
            }),
            None => None,
        };
        let output = match &self.output {
            Some((choice, models)) => Some(VoiceOutput {
                models: Arc::clone(models) as Arc<dyn OutputModels>,
                config: serde_json::from_value::<OutputConfig>(choice.config.clone())
                    .map_err(|e| invalid("output", e))?,
            }),
            None => None,
        };
        let config = serde_json::from_value::<VoiceConfig>(self.config.clone()).map_err(|e| invalid("call", e))?;
        Ok(VoiceSetup { input, output, config })
    }

    /// Loads every slot the halves use, all at once, as a call starting on them would. What loaded is held by the
    /// answer even when a slot did not (its code is the second): the others stay loaded for the next ask.
    pub(crate) async fn load(&self) -> (Loaded, Option<String>) {
        let input = async {
            match &self.input {
                None => (None, None, None),
                Some((choice, models)) => {
                    let ends_turns = async {
                        if choice.end_of_turn.is_some() {
                            Some(models.end_of_turn().await)
                        } else {
                            None
                        }
                    };
                    let (vad, transcriber, end_of_turn) = tokio::join!(models.vad(), models.transcriber(), ends_turns);
                    (Some(vad), Some(transcriber), end_of_turn)
                }
            }
        };
        let output = async {
            match &self.output {
                None => None,
                Some((_, models)) => Some(models.speaker().await),
            }
        };
        let ((vad, transcriber, end_of_turn), speaker) = tokio::join!(input, output);
        let mut failed = None;
        let loaded = Loaded {
            _vad: kept(vad, &mut failed),
            _transcriber: kept(transcriber, &mut failed),
            _end_of_turn: kept(end_of_turn, &mut failed),
            _speaker: kept(speaker, &mut failed),
        };
        (loaded, failed)
    }
}

/// The model a load gave, or, when it failed, nothing, and the first code to have failed noted in `failed`.
fn kept<T>(result: Option<Result<T, String>>, failed: &mut Option<String>) -> Option<T> {
    match result {
        Some(Ok(model)) => Some(model),
        Some(Err(code)) => {
            failed.get_or_insert(code);
            None
        }
        None => None,
    }
}
