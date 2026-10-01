//! What the native engine answers when it will not or cannot do something, in the shape of sidevoice-core's
//! refusals: a stable `key` the page translates, the parameters its message needs (beside it, flat), and the
//! sentence in English for a client that does not know the key. Every key is made here, and listed in
//! docs/BRIDGE.md → "Refusals"; a new one goes in both.

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Error {
    pub key: &'static str,
    #[serde(flatten)]
    pub params: BTreeMap<&'static str, Value>,
    pub message: String,
}

impl Error {
    fn new(key: &'static str, message: impl Into<String>) -> Self {
        Error { key, params: BTreeMap::new(), message: message.into() }
    }

    fn with(mut self, name: &'static str, value: impl Into<Value>) -> Self {
        self.params.insert(name, value.into());
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// The call itself is malformed (a missing header, audio not sent as bytes). A bug in the caller, not the person's.
pub fn bad_request(detail: impl Into<String>) -> Error {
    Error::new("bad_request", detail)
}

/// Something inside the app went wrong (a poisoned lock, a worker thread that died).
pub fn internal(detail: impl fmt::Display) -> Error {
    Error::new("internal", format!("The native engine failed: {detail}"))
}

/// An engine this app has no runtime for: unknown, a page's engine, or one a newer catalogue added.
pub fn engine_unsupported(engine: &str) -> Error {
    Error::new("engine_unsupported", format!("This app cannot run the engine {engine}.")).with("engine", engine)
}

pub fn engine_no_package(engine: &str, platform: &str) -> Error {
    Error::new("engine_no_package", format!("{engine} has no package for this device ({platform})."))
        .with("engine", engine)
        .with("platform", platform)
}

pub fn family_unsupported(engine: &str, family: &str) -> Error {
    Error::new("family_unsupported", format!("This app does not run {family} models on {engine}."))
        .with("engine", engine)
        .with("family", family)
}

pub fn model_unknown(model: &str) -> Error {
    Error::new("model_unknown", format!("Unknown model {model}.")).with("model", model)
}

pub fn build_missing(model: &str, engine: &str) -> Error {
    Error::new("build_missing", format!("{model} has no build for {engine}."))
        .with("model", model)
        .with("engine", engine)
}

/// The build exists but not for this device: a feature it needs, or an accelerator it can use, is missing here.
pub fn build_unfit(model: &str, engine: &str) -> Error {
    Error::new("build_unfit", format!("{model} on {engine} does not run on this device."))
        .with("model", model)
        .with("engine", engine)
}

pub fn model_needs_memory(model: &str, needed_mb: u64, memory_mb: u64) -> Error {
    Error::new("model_needs_memory", format!("{model} needs {needed_mb} MB of memory; this device has {memory_mb} MB."))
        .with("model", model)
        .with("needed_mb", needed_mb)
        .with("memory_mb", memory_mb)
}

pub fn model_wrong_task(model: &str, task: &str) -> Error {
    Error::new("model_wrong_task", format!("{model} is not a {task} model.")).with("model", model).with("task", task)
}

pub fn accelerator_unusable(model: &str, engine: &str, accelerator: &str, usable: Vec<String>) -> Error {
    Error::new(
        "accelerator_unusable",
        format!("{model} on {engine} cannot use {accelerator} here; it can use {}.", usable.join(", ")),
    )
    .with("model", model)
    .with("engine", engine)
    .with("accelerator", accelerator)
    .with("usable", usable)
}

pub fn not_installed(model: &str, engine: &str) -> Error {
    Error::new("not_installed", format!("{model} on {engine} is not downloaded yet."))
        .with("model", model)
        .with("engine", engine)
}

pub fn voice_unknown(model: &str, voice: &str) -> Error {
    Error::new("voice_unknown", format!("{model} has no voice {voice}.")).with("model", model).with("voice", voice)
}

pub fn language_unsupported(language: &str) -> Error {
    Error::new("language_unsupported", format!("The model does not know the language {language:?}."))
        .with("language", language)
}

pub fn download_refused(url: &str) -> Error {
    Error::new("download_refused", format!("Refusing a download that is not https: {url}")).with("url", url)
}

pub fn download_failed(url: &str, detail: impl fmt::Display) -> Error {
    Error::new("download_failed", format!("Could not download {url}: {detail}")).with("url", url)
}

/// The bytes downloaded are not the ones the catalogue names (SHA-256).
pub fn download_corrupt(url: &str) -> Error {
    Error::new("download_corrupt", format!("{url} is not the file the catalogue names.")).with("url", url)
}

/// Unpacking or moving a download into place failed (disk full, permissions, an archive without its files).
pub fn install_failed(detail: impl fmt::Display) -> Error {
    Error::new("install_failed", format!("Could not install the download: {detail}"))
}

/// The engine refused to load or to run (a library that fails its hash, a model the runtime rejects).
pub fn runtime_failed(engine: &str, detail: impl fmt::Display) -> Error {
    Error::new("runtime_failed", format!("{engine} failed: {detail}")).with("engine", engine)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_refusal_is_its_key_its_parameters_flat_and_an_english_message() {
        let json = serde_json::to_value(super::not_installed("whisper-small", "sherpa-onnx")).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "key": "not_installed",
                "model": "whisper-small",
                "engine": "sherpa-onnx",
                "message": "whisper-small on sherpa-onnx is not downloaded yet."
            })
        );
    }
}
