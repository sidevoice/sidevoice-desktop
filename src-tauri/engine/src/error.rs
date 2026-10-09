//! What the native engine answers when it will not or cannot do something, in the shape of sidevoice-core's
//! refusals: a stable `key` the page translates, the parameters its message needs (beside it, flat), and the
//! sentence in English for a client that does not know the key. Every key is made here, and listed in
//! docs/BRIDGE.md → "Refusals"; a new one goes in both. sidevoice-engine fails with stable codes of its own
//! (`digest-mismatch`, `model-load-failed`…): `engine_failed` turns each into one of these keys, with the engine's
//! code beside it as `code`.

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

pub fn model_unknown(model: &str) -> Error {
    Error::new("model_unknown", format!("Unknown model {model}.")).with("model", model)
}

pub fn build_missing(model: &str, engine: &str) -> Error {
    Error::new("build_missing", format!("{model} has no build for {engine}."))
        .with("model", model)
        .with("engine", engine)
}

/// The build exists but not for this device: a feature it needs, or an accelerator it can use, is missing here.
/// `reason`: the engine's code for why, when it gave one (`build-accelerator`, `no-runtime-for-platform`…).
pub fn build_unfit(model: &str, engine: &str, reason: Option<&str>) -> Error {
    let refusal = Error::new("build_unfit", format!("{model} on {engine} does not run on this device."))
        .with("model", model)
        .with("engine", engine);
    match reason {
        Some(reason) => refusal.with("reason", reason),
        None => refusal,
    }
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

pub fn not_installed(model: &str, engine: &str) -> Error {
    Error::new("not_installed", format!("{model} on {engine} is not downloaded yet."))
        .with("model", model)
        .with("engine", engine)
}

pub fn download_failed(model: &str, engine: &str) -> Error {
    Error::new("download_failed", format!("Could not download {model} on {engine}."))
        .with("model", model)
        .with("engine", engine)
}

/// The bytes downloaded are not the ones the catalogue names (SHA-256).
pub fn download_corrupt(model: &str, engine: &str) -> Error {
    Error::new("download_corrupt", format!("A file of {model} on {engine} is not the one the catalogue names."))
        .with("model", model)
        .with("engine", engine)
}

/// Unpacking or moving a download into place failed (disk full, permissions, an archive without its files).
pub fn install_failed(detail: impl fmt::Display) -> Error {
    Error::new("install_failed", format!("Could not install the download: {detail}"))
}

/// The page cancelled the install (`cancel(job)`): what it was downloading is gone from disk.
pub fn install_cancelled() -> Error {
    Error::new("install_cancelled", "The download was cancelled.")
}

/// The engine refused to load or to run (a model the runtime rejects, a transcription that failed).
pub fn runtime_failed(engine: &str, detail: impl fmt::Display) -> Error {
    Error::new("runtime_failed", format!("{engine} failed: {detail}")).with("engine", engine)
}

/// What sidevoice-engine failed with, doing something with `model` on `engine`, as the page's key; the engine's own
/// code goes with it as `code`. Codes it adds later fall to `runtime_failed`, still with their `code`.
pub fn engine_failed(error: sidevoice_engine::Error, model: &str, engine: &str) -> Error {
    let code = error.code;
    let refusal = match code {
        "cancelled" => return install_cancelled(),
        "model-not-found" => model_unknown(model),
        "build-not-found" => build_missing(model, engine),
        "backend-not-in-this-build" => engine_unsupported(engine),
        "no-build-available" | "no-runtime-for-platform" | "no-accelerator" | "build-accelerator" => {
            build_unfit(model, engine, None)
        }
        "download-failed" => download_failed(model, engine),
        "digest-mismatch" => download_corrupt(model, engine),
        "file-not-installed" => not_installed(model, engine),
        "unknown-voice" => {
            Error::new("voice_unknown", format!("{model} does not have that voice.")).with("model", model)
        }
        "model-cannot-transcribe" => model_wrong_task(model, "speech-to-text"),
        "model-cannot-speak" => model_wrong_task(model, "text-to-speech"),
        code if code.starts_with("archive-")
            || code.starts_with("storage-")
            || code.starts_with("file-")
            || matches!(code, "digest-invalid" | "artifact-key-conflict") =>
        {
            install_failed(code)
        }
        _ => runtime_failed(engine, code),
    };
    refusal.with("code", code)
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

    #[test]
    fn an_engine_failure_is_one_of_the_page_s_keys_with_the_engine_s_code_beside_it() {
        let failed = |code| super::engine_failed(sidevoice_engine::Error::new(code), "whisper-base", "sherpa-onnx");
        assert_eq!(failed("cancelled"), super::install_cancelled());
        for (code, key) in [
            ("model-not-found", "model_unknown"),
            ("digest-mismatch", "download_corrupt"),
            ("download-failed", "download_failed"),
            ("archive-corrupt", "install_failed"),
            ("storage-failed", "install_failed"),
            ("model-load-failed", "runtime_failed"),
            ("transcription-failed", "runtime_failed"),
            ("a-code-from-a-later-engine", "runtime_failed"),
        ] {
            let error = failed(code);
            assert_eq!((error.key, &error.params["code"]), (key, &serde_json::json!(code)), "{code}");
        }
    }
}
