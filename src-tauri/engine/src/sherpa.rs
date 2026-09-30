//! sherpa-onnx, loaded at run time from a downloaded package (never linked into the app).
//!
//! The package's libraries are opened with `dlopen` in the catalog's order (ONNX Runtime first, so the C API
//! library finds it already loaded), and the handful of C functions used here are looked up by name. The
//! structs come from `sherpa_ffi.rs`, generated from the same version's header.

use crate::sherpa_ffi as ffi;
use libloading::Library;
use serde::Deserialize;
use std::ffi::{c_void, CStr, CString};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub type Error = String;

type CreateRecognizer = unsafe extern "C" fn(*const ffi::SherpaOnnxOfflineRecognizerConfig) -> *const c_void;
type DestroyRecognizer = unsafe extern "C" fn(*const c_void);
type CreateStream = unsafe extern "C" fn(*const c_void) -> *const c_void;
type DestroyStream = unsafe extern "C" fn(*const c_void);
type AcceptWaveform = unsafe extern "C" fn(*const c_void, i32, *const f32, i32);
type Decode = unsafe extern "C" fn(*const c_void, *const c_void);
type ResultJson = unsafe extern "C" fn(*const c_void) -> *const c_char;
type DestroyJson = unsafe extern "C" fn(*const c_char);
type CreateTts = unsafe extern "C" fn(*const ffi::SherpaOnnxOfflineTtsConfig) -> *const c_void;
type DestroyTts = unsafe extern "C" fn(*const c_void);
type TtsSampleRate = unsafe extern "C" fn(*const c_void) -> i32;
type Progress = Option<unsafe extern "C" fn(*const f32, i32, f32, *mut c_void) -> i32>;
type Generate = unsafe extern "C" fn(
    *const c_void,
    *const c_char,
    *const ffi::SherpaOnnxGenerationConfig,
    Progress,
    *mut c_void,
) -> *const ffi::SherpaOnnxGeneratedAudio;
type DestroyAudio = unsafe extern "C" fn(*const ffi::SherpaOnnxGeneratedAudio);
type Version = unsafe extern "C" fn() -> *const c_char;

/// The loaded engine. Keeps its libraries open for as long as it lives.
pub struct Sherpa {
    create_recognizer: CreateRecognizer,
    destroy_recognizer: DestroyRecognizer,
    create_stream: CreateStream,
    destroy_stream: DestroyStream,
    accept_waveform: AcceptWaveform,
    decode: Decode,
    result_json: ResultJson,
    destroy_json: DestroyJson,
    create_tts: CreateTts,
    destroy_tts: DestroyTts,
    tts_sample_rate: TtsSampleRate,
    generate: Generate,
    destroy_audio: DestroyAudio,
    pub version: String,
    _libraries: Vec<Library>,
}

// The function pointers are plain C functions; the handles they create are guarded where used.
unsafe impl Send for Sherpa {}
unsafe impl Sync for Sherpa {}

fn symbol<T: Copy>(library: &Library, name: &str) -> Result<T, Error> {
    // SAFETY: the caller names the C signature the header declares for `name` (see the type aliases above).
    unsafe { library.get::<T>(name.as_bytes()).map(|s| *s).map_err(|e| format!("{name}: {e}")) }
}

impl Sherpa {
    /// Opens the package's libraries (relative to `root`, in order); the C API is the last one. Each file is
    /// hashed right before it is opened and must match the catalog: the app loads libraries with library
    /// validation off (an ad-hoc signed build), so this check is what stands between a file swapped on disk
    /// and code running with the app's microphone permission.
    pub fn load(root: &Path, libraries: &[sidevoice_desktop_core::engines::Library]) -> Result<Sherpa, Error> {
        let mut opened = Vec::new();
        for library in libraries {
            let path = root.join(&library.path);
            let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let got = crate::install::hex(&<sha2::Sha256 as sha2::Digest>::digest(&bytes));
            if got != library.sha256.to_ascii_lowercase() {
                return Err(format!("{} is not the engine the catalog names (SHA-256 {got}); remove the engines folder to download it again", path.display()));
            }
            // SAFETY: loading a library runs its initialisers; these are the pinned, hash-checked engine's own.
            let library = unsafe { Library::new(&path) }.map_err(|e| format!("cannot load {}: {e}", path.display()))?;
            opened.push(library);
        }
        let api = opened.last().ok_or("the engine package names no library")?;
        let version_fn: Version = symbol(api, "SherpaOnnxGetVersionStr")?;
        // SAFETY: returns a static NUL-terminated string.
        let version = unsafe { CStr::from_ptr(version_fn()) }.to_string_lossy().into_owned();
        if version != ffi::HEADER_VERSION {
            return Err(format!(
                "engine library is sherpa-onnx {version}, its bindings are for {}",
                ffi::HEADER_VERSION
            ));
        }
        Ok(Sherpa {
            create_recognizer: symbol(api, "SherpaOnnxCreateOfflineRecognizer")?,
            destroy_recognizer: symbol(api, "SherpaOnnxDestroyOfflineRecognizer")?,
            create_stream: symbol(api, "SherpaOnnxCreateOfflineStream")?,
            destroy_stream: symbol(api, "SherpaOnnxDestroyOfflineStream")?,
            accept_waveform: symbol(api, "SherpaOnnxAcceptWaveformOffline")?,
            decode: symbol(api, "SherpaOnnxDecodeOfflineStream")?,
            result_json: symbol(api, "SherpaOnnxGetOfflineStreamResultAsJson")?,
            destroy_json: symbol(api, "SherpaOnnxDestroyOfflineStreamResultJson")?,
            create_tts: symbol(api, "SherpaOnnxCreateOfflineTts")?,
            destroy_tts: symbol(api, "SherpaOnnxDestroyOfflineTts")?,
            tts_sample_rate: symbol(api, "SherpaOnnxOfflineTtsSampleRate")?,
            generate: symbol(api, "SherpaOnnxOfflineTtsGenerateWithConfig")?,
            destroy_audio: symbol(api, "SherpaOnnxDestroyOfflineTtsGeneratedAudio")?,
            version,
            _libraries: opened,
        })
    }
}

/// Keeps the C strings a config points at alive until the engine has read them.
#[derive(Default)]
struct Strings(Vec<CString>);

impl Strings {
    fn keep(&mut self, value: &str) -> Result<*const c_char, Error> {
        let c = CString::new(value).map_err(|_| format!("NUL in {value:?}"))?;
        let pointer = c.as_ptr();
        self.0.push(c);
        Ok(pointer)
    }

    fn path(&mut self, dir: &Path, file: &str) -> Result<*const c_char, Error> {
        let full: PathBuf = dir.join(file);
        if !full.exists() {
            return Err(format!("model file missing: {}", full.display()));
        }
        self.keep(&full.to_string_lossy())
    }
}

/// `builds["sherpa-onnx"].config` of a Whisper model in the catalog.
#[derive(Debug, Clone, Deserialize)]
pub struct WhisperFiles {
    pub encoder: String,
    pub decoder: String,
    pub tokens: String,
}

/// `builds["sherpa-onnx"].config` of a Kokoro model in the catalog.
#[derive(Debug, Clone, Deserialize)]
pub struct KokoroFiles {
    pub model: String,
    pub voices: String,
    pub tokens: String,
    pub data_dir: String,
    #[serde(default)]
    pub dict_dir: Option<String>,
    /// Comma-separated, relative to the model directory.
    #[serde(default)]
    pub lexicon: Option<String>,
}

pub struct Recognizer {
    engine: Arc<Sherpa>,
    handle: *const c_void,
    lock: Mutex<()>,
}

unsafe impl Send for Recognizer {}
unsafe impl Sync for Recognizer {}

impl Recognizer {
    /// `language`: Whisper's code (`es`, `en`…) or empty for auto-detection. `provider`: `cpu` or `coreml`.
    pub fn whisper(
        engine: Arc<Sherpa>,
        dir: &Path,
        files: &WhisperFiles,
        language: &str,
        provider: &str,
        threads: i32,
    ) -> Result<Self, Error> {
        let mut s = Strings::default();
        let mut config = ffi::SherpaOnnxOfflineRecognizerConfig::default();
        config.feat_config.sample_rate = 16_000;
        config.feat_config.feature_dim = 80;
        config.model_config.whisper.encoder = s.path(dir, &files.encoder)?;
        config.model_config.whisper.decoder = s.path(dir, &files.decoder)?;
        config.model_config.whisper.language = s.keep(language)?;
        config.model_config.whisper.task = s.keep("transcribe")?;
        config.model_config.whisper.tail_paddings = -1;
        config.model_config.tokens = s.path(dir, &files.tokens)?;
        config.model_config.num_threads = threads;
        config.model_config.provider = s.keep(provider)?;
        config.model_config.model_type = s.keep("whisper")?;
        config.decoding_method = s.keep("greedy_search")?;
        // SAFETY: `config` and every string it points at outlive the call; the engine copies what it keeps.
        let handle = unsafe { (engine.create_recognizer)(&config) };
        if handle.is_null() {
            return Err(format!("sherpa-onnx refused the Whisper model in {} ({provider})", dir.display()));
        }
        Ok(Recognizer { engine, handle, lock: Mutex::new(()) })
    }

    /// Mono samples in [-1, 1] at `sample_rate` (resampled by the engine) → text.
    pub fn transcribe(&self, samples: &[f32], sample_rate: i32) -> Result<String, Error> {
        let _one_at_a_time = self.lock.lock().map_err(|_| "recognizer poisoned")?;
        let length = i32::try_from(samples.len()).map_err(|_| "audio too long")?;
        // SAFETY: handles come from this engine and are destroyed below; `samples` outlives the calls.
        unsafe {
            let stream = (self.engine.create_stream)(self.handle);
            if stream.is_null() {
                return Err("could not create a stream".into());
            }
            (self.engine.accept_waveform)(stream, sample_rate, samples.as_ptr(), length);
            (self.engine.decode)(self.handle, stream);
            let json = (self.engine.result_json)(stream);
            let text = if json.is_null() {
                Err("no result".to_string())
            } else {
                let parsed: Result<serde_json::Value, _> = serde_json::from_slice(CStr::from_ptr(json).to_bytes());
                (self.engine.destroy_json)(json);
                parsed.map_err(|e| e.to_string()).map(|v| v["text"].as_str().unwrap_or("").trim().to_string())
            };
            (self.engine.destroy_stream)(stream);
            text
        }
    }
}

impl Drop for Recognizer {
    fn drop(&mut self) {
        // SAFETY: created by this engine, destroyed once.
        unsafe { (self.engine.destroy_recognizer)(self.handle) }
    }
}

pub struct Audio {
    pub samples: Vec<f32>,
    pub sample_rate: i32,
}

pub struct Tts {
    engine: Arc<Sherpa>,
    handle: *const c_void,
    lock: Mutex<()>,
}

unsafe impl Send for Tts {}
unsafe impl Sync for Tts {}

impl Tts {
    /// `language`: the phonemizer's language for this model (Kokoro multi-lang: `es`, `en-us`…).
    pub fn kokoro(
        engine: Arc<Sherpa>,
        dir: &Path,
        files: &KokoroFiles,
        language: &str,
        provider: &str,
        threads: i32,
    ) -> Result<Self, Error> {
        let mut s = Strings::default();
        let mut config = ffi::SherpaOnnxOfflineTtsConfig::default();
        let kokoro = &mut config.model.kokoro;
        kokoro.model = s.path(dir, &files.model)?;
        kokoro.voices = s.path(dir, &files.voices)?;
        kokoro.tokens = s.path(dir, &files.tokens)?;
        kokoro.data_dir = s.path(dir, &files.data_dir)?;
        kokoro.length_scale = 1.0;
        if let Some(dict) = &files.dict_dir {
            kokoro.dict_dir = s.path(dir, dict)?;
        }
        if let Some(lexicon) = &files.lexicon {
            let joined: Vec<String> =
                lexicon.split(',').map(|f| dir.join(f.trim()).to_string_lossy().into_owned()).collect();
            kokoro.lexicon = s.keep(&joined.join(","))?;
        }
        kokoro.lang = s.keep(language)?;
        config.model.num_threads = threads;
        config.model.provider = s.keep(provider)?;
        config.max_num_sentences = 1;
        // SAFETY: as for the recognizer.
        let handle = unsafe { (engine.create_tts)(&config) };
        if handle.is_null() {
            return Err(format!("sherpa-onnx refused the Kokoro model in {} ({provider})", dir.display()));
        }
        Ok(Tts { engine, handle, lock: Mutex::new(()) })
    }

    pub fn sample_rate(&self) -> i32 {
        // SAFETY: a live handle.
        unsafe { (self.engine.tts_sample_rate)(self.handle) }
    }

    pub fn synthesize(&self, text: &str, sid: i32, speed: f32) -> Result<Audio, Error> {
        let _one_at_a_time = self.lock.lock().map_err(|_| "tts poisoned")?;
        let text = CString::new(text).map_err(|_| "NUL in text")?;
        let config = ffi::SherpaOnnxGenerationConfig { sid, speed, silence_scale: 0.2, ..Default::default() };
        // SAFETY: live handle; `text` and `config` outlive the call; the audio is copied, then destroyed.
        unsafe {
            let audio = (self.engine.generate)(self.handle, text.as_ptr(), &config, None, std::ptr::null_mut());
            if audio.is_null() {
                return Err("sherpa-onnx produced no audio".into());
            }
            let generated = &*audio;
            let samples = if generated.samples.is_null() || generated.n <= 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(generated.samples, generated.n as usize).to_vec()
            };
            let sample_rate = generated.sample_rate;
            (self.engine.destroy_audio)(audio);
            Ok(Audio { samples, sample_rate })
        }
    }
}

impl Drop for Tts {
    fn drop(&mut self) {
        // SAFETY: created by this engine, destroyed once.
        unsafe { (self.engine.destroy_tts)(self.handle) }
    }
}
