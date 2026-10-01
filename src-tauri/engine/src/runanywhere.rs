//! SPIKE (spike/runanywhere, never merged): RunAnywhere's RACommons as a native engine, loaded at run time like
//! sherpa-onnx (docs/ENGINES.md) — evaluation only, see /workspace/sidevoice/research/RUNANYWHERE-SPIKE.md.
//!
//! Pinned: RunanywhereAI/runanywhere-sdks v0.20.37 (a18d1d36a3f2b1935e4de83194d0e83dfe669a2c). RunAnywhere ships no
//! Rust binding and no crate: this talks to its C ABI (`rac_*`, headers in `RACommons-linux-x86_64-v0.20.37`) through
//! hand-written `repr(C)` mirrors of the handful of structs used. The package is RACommons' shared libraries plus
//! the `libsherpa-onnx-c-api.so` its sherpa backend needs (RACommons does not ship it; the cpp-desktop kit does).
//!
//! RACommons needs two things from its host before it does anything: a platform adapter (files, log, clock,
//! memory) and, for any network use, an HTTP transport. Both are implemented here in Rust; the transport records
//! every URL it is asked for, so a run shows whether the library phones home.

use crate::{error, Audio, Recognize, Speak};
use libloading::Library;
use sidevoice_desktop_core::engines::{Capability, Package};
use std::ffi::{c_void, CStr, CString};
use std::io::Read;
use std::os::raw::c_char;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

pub const ENGINE: &str = "runanywhere";
pub const PINNED: &str = "0.20.37";

type Res = i32;
const RAC_SUCCESS: Res = 0;
const RAC_ERROR_FILE_NOT_FOUND: Res = -183;
const RAC_ERROR_FILE_WRITE_FAILED: Res = -185;
const RAC_ERROR_NOT_SUPPORTED: Res = -236;
const RAC_ERROR_NETWORK_ERROR: Res = -151;
const RAC_ERROR_CANCELLED: Res = -380;
const RAC_PLATFORM_ADAPTER_ABI_VERSION: u32 = 1;
const RAC_LOG_WARNING: i32 = 3;
const RAC_AUDIO_FORMAT_PCM: i32 = 0;

// ---------------------------------------------------------------------------------------------------------------
// C ABI mirrors (rac_platform_adapter.h, rac_core.h, rac_http_*.h, rac_stt_types.h, rac_tts_types.h)
// ---------------------------------------------------------------------------------------------------------------

#[repr(C)]
pub struct MemoryInfo {
    total_bytes: u64,
    available_bytes: u64,
    used_bytes: u64,
}

type Unused = Option<unsafe extern "C" fn()>;

#[repr(C)]
pub struct PlatformAdapter {
    abi_version: u32,
    struct_size: u32,
    file_exists: Option<unsafe extern "C" fn(*const c_char, *mut c_void) -> i32>,
    file_read: Option<unsafe extern "C" fn(*const c_char, *mut *mut c_void, *mut usize, *mut c_void) -> Res>,
    file_write: Option<unsafe extern "C" fn(*const c_char, *const c_void, usize, *mut c_void) -> Res>,
    file_delete: Option<unsafe extern "C" fn(*const c_char, *mut c_void) -> Res>,
    secure_get: Option<unsafe extern "C" fn(*const c_char, *mut *mut c_char, *mut c_void) -> Res>,
    secure_set: Option<unsafe extern "C" fn(*const c_char, *const c_char, *mut c_void) -> Res>,
    secure_delete: Option<unsafe extern "C" fn(*const c_char, *mut c_void) -> Res>,
    log: Option<unsafe extern "C" fn(i32, *const c_char, *const c_char, *mut c_void)>,
    now_ms: Option<unsafe extern "C" fn(*mut c_void) -> i64>,
    get_memory_info: Option<unsafe extern "C" fn(*mut MemoryInfo, *mut c_void) -> Res>,
    http_download: Unused,
    http_download_cancel: Unused,
    extract_archive: Unused,
    file_list_directory: Unused,
    is_non_empty_directory: Option<unsafe extern "C" fn(*const c_char, *mut c_void) -> i32>,
    get_vendor_id: Unused,
    user_data: *mut c_void,
}

#[repr(C)]
pub struct Config {
    platform_adapter: *const PlatformAdapter,
    log_level: i32,
    log_tag: *const c_char,
    reserved: *mut c_void,
}

#[repr(C)]
pub struct HeaderKv {
    name: *const c_char,
    value: *const c_char,
}

#[repr(C)]
pub struct HttpRequest {
    method: *const c_char,
    url: *const c_char,
    headers: *const HeaderKv,
    header_count: usize,
    body_bytes: *const u8,
    body_len: usize,
    timeout_ms: i32,
    follow_redirects: i32,
    expected_checksum_hex: *const c_char,
}

#[repr(C)]
pub struct HttpResponse {
    status: i32,
    headers: *mut HeaderKv,
    header_count: usize,
    body_bytes: *mut u8,
    body_len: usize,
    redirected_url: *mut c_char,
    elapsed_ms: u64,
}

type ChunkFn = unsafe extern "C" fn(*const u8, usize, u64, u64, *mut c_void) -> i32;

#[repr(C)]
pub struct TransportOps {
    request_send: Option<unsafe extern "C" fn(*mut c_void, *const HttpRequest, *mut HttpResponse) -> Res>,
    request_stream:
        Option<unsafe extern "C" fn(*mut c_void, *const HttpRequest, ChunkFn, *mut c_void, *mut HttpResponse) -> Res>,
    request_resume: Option<
        unsafe extern "C" fn(*mut c_void, *const HttpRequest, u64, ChunkFn, *mut c_void, *mut HttpResponse) -> Res,
    >,
    init: Option<unsafe extern "C" fn(*mut c_void) -> Res>,
    destroy: Option<unsafe extern "C" fn(*mut c_void)>,
}

#[repr(C)]
pub struct DownloadRequest {
    pub url: *const c_char,
    pub destination_path: *const c_char,
    pub headers: *const HeaderKv,
    pub header_count: usize,
    pub timeout_ms: i32,
    pub follow_redirects: i32,
    pub resume_from_byte: u64,
    pub expected_sha256_hex: *const c_char,
}

#[repr(C)]
struct SttOptions {
    language: *const c_char,
    detect_language: i32,
    enable_punctuation: i32,
    enable_diarization: i32,
    max_speakers: i32,
    enable_timestamps: i32,
    audio_format: i32,
    sample_rate: i32,
}

#[repr(C)]
struct SttResult {
    text: *mut c_char,
    detected_language: *mut c_char,
    words: *mut c_void,
    num_words: usize,
    confidence: f32,
    processing_time_ms: i64,
}

#[repr(C)]
struct TtsOptions {
    voice: *const c_char,
    language: *const c_char,
    rate: f32,
    pitch: f32,
    volume: f32,
    audio_format: i32,
    sample_rate: i32,
    use_ssml: i32,
}

#[repr(C)]
struct TtsResult {
    audio_data: *mut c_void,
    audio_size: usize,
    audio_format: i32,
    sample_rate: i32,
    duration_ms: i64,
    processing_time_ms: i64,
}

type Handle = *mut c_void;
type DownloadProgress = unsafe extern "C" fn(u64, u64, *mut c_void) -> i32;

/// The `rac_*` functions this adapter calls, looked up by name in the loaded libraries.
struct Api {
    init: unsafe extern "C" fn(*const Config) -> Res,
    error_message: unsafe extern "C" fn(Res) -> *const c_char,
    error_details: unsafe extern "C" fn() -> *const c_char,
    sdk_version: unsafe extern "C" fn() -> *const c_char,
    transport_register: unsafe extern "C" fn(*const TransportOps, *mut c_void) -> Res,
    download_execute: unsafe extern "C" fn(*const DownloadRequest, Option<DownloadProgress>, *mut c_void, *mut i32) -> i32,
    backend_sherpa_register: unsafe extern "C" fn() -> Res,
    stt_create: unsafe extern "C" fn(*mut Handle) -> Res,
    stt_load: unsafe extern "C" fn(Handle, *const c_char, *const c_char, *const c_char) -> Res,
    stt_transcribe: unsafe extern "C" fn(Handle, *const c_void, usize, *const SttOptions, *mut SttResult) -> Res,
    stt_result_free: unsafe extern "C" fn(*mut SttResult),
    stt_unload: unsafe extern "C" fn(Handle) -> Res,
    stt_destroy: unsafe extern "C" fn(Handle),
    tts_create: unsafe extern "C" fn(*mut Handle) -> Res,
    tts_load: unsafe extern "C" fn(Handle, *const c_char, *const c_char, *const c_char) -> Res,
    tts_synthesize: unsafe extern "C" fn(Handle, *const c_char, *const TtsOptions, *mut TtsResult) -> Res,
    tts_result_free: unsafe extern "C" fn(*mut TtsResult),
    tts_unload: unsafe extern "C" fn(Handle) -> Res,
    tts_destroy: unsafe extern "C" fn(Handle),
}

fn symbol<T: Copy>(libraries: &[Library], name: &str) -> Result<T, String> {
    for library in libraries.iter().rev() {
        // SAFETY: `T` is the C signature the RACommons v0.20.37 header declares for `name`.
        if let Ok(s) = unsafe { library.get::<T>(name.as_bytes()) } {
            return Ok(*s);
        }
    }
    Err(format!("{name}: not exported by the RunAnywhere libraries"))
}

// ---------------------------------------------------------------------------------------------------------------
// Platform adapter and HTTP transport, in Rust
// ---------------------------------------------------------------------------------------------------------------

fn path_of(p: *const c_char) -> Option<String> {
    // SAFETY: RACommons passes NUL-terminated paths.
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

unsafe extern "C" fn file_exists(p: *const c_char, _: *mut c_void) -> i32 {
    path_of(p).is_some_and(|p| Path::new(&p).exists()) as i32
}

unsafe extern "C" fn file_read(p: *const c_char, out: *mut *mut c_void, size: *mut usize, _: *mut c_void) -> Res {
    let Some(Ok(bytes)) = path_of(p).map(std::fs::read) else { return RAC_ERROR_FILE_NOT_FOUND };
    let buffer = libc::malloc(bytes.len().max(1));
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer as *mut u8, bytes.len());
    *out = buffer;
    *size = bytes.len();
    RAC_SUCCESS
}

unsafe extern "C" fn file_write(p: *const c_char, data: *const c_void, size: usize, _: *mut c_void) -> Res {
    let Some(p) = path_of(p) else { return RAC_ERROR_FILE_WRITE_FAILED };
    let bytes = std::slice::from_raw_parts(data as *const u8, size);
    match std::fs::write(p, bytes) {
        Ok(()) => RAC_SUCCESS,
        Err(_) => RAC_ERROR_FILE_WRITE_FAILED,
    }
}

unsafe extern "C" fn file_delete(p: *const c_char, _: *mut c_void) -> Res {
    let _ = path_of(p).map(std::fs::remove_file);
    RAC_SUCCESS
}

unsafe extern "C" fn secure_get(_: *const c_char, _: *mut *mut c_char, _: *mut c_void) -> Res {
    RAC_ERROR_NOT_SUPPORTED
}

unsafe extern "C" fn secure_set(_: *const c_char, _: *const c_char, _: *mut c_void) -> Res {
    RAC_ERROR_NOT_SUPPORTED
}

unsafe extern "C" fn secure_delete(_: *const c_char, _: *mut c_void) -> Res {
    RAC_ERROR_NOT_SUPPORTED
}

unsafe extern "C" fn log(level: i32, category: *const c_char, message: *const c_char, _: *mut c_void) {
    if level >= RAC_LOG_WARNING || std::env::var_os("SPIKE_RAC_LOG").is_some() {
        eprintln!("[rac {level}] {}: {}", path_of(category).unwrap_or_default(), path_of(message).unwrap_or_default());
    }
}

unsafe extern "C" fn now_ms(_: *mut c_void) -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

unsafe extern "C" fn memory_info(out: *mut MemoryInfo, _: *mut c_void) -> Res {
    let total = crate::memory::total_mb().unwrap_or(0) * 1024 * 1024;
    *out = MemoryInfo { total_bytes: total, available_bytes: total, used_bytes: 0 };
    RAC_SUCCESS
}

unsafe extern "C" fn non_empty_dir(p: *const c_char, _: *mut c_void) -> i32 {
    path_of(p).and_then(|p| std::fs::read_dir(p).ok()).is_some_and(|mut d| d.next().is_some()) as i32
}

/// Every URL RACommons asked the transport for, in order (for the "does it phone home" check).
pub fn requested_urls() -> Vec<String> {
    URLS.get_or_init(Default::default).lock().map(|u| u.clone()).unwrap_or_default()
}

static URLS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn http() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| reqwest::blocking::Client::builder().timeout(None).build().expect("http client"))
}

unsafe fn request_of(req: *const HttpRequest, resume: u64) -> Result<reqwest::blocking::Response, Res> {
    let req = &*req;
    let url = path_of(req.url).ok_or(RAC_ERROR_NETWORK_ERROR)?;
    let method = path_of(req.method).unwrap_or_else(|| "GET".into());
    URLS.get_or_init(Default::default).lock().map(|mut u| u.push(format!("{method} {url}"))).ok();
    let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| RAC_ERROR_NETWORK_ERROR)?;
    let mut builder = http().request(method, &url);
    for i in 0..req.header_count {
        let kv = &*req.headers.add(i);
        if let (Some(n), Some(v)) = (path_of(kv.name), path_of(kv.value)) {
            builder = builder.header(n, v);
        }
    }
    if resume > 0 {
        builder = builder.header("Range", format!("bytes={resume}-"));
    }
    if !req.body_bytes.is_null() && req.body_len > 0 {
        builder = builder.body(std::slice::from_raw_parts(req.body_bytes, req.body_len).to_vec());
    }
    builder.send().map_err(|_| RAC_ERROR_NETWORK_ERROR)
}

unsafe fn meta(resp: &reqwest::blocking::Response, out: *mut HttpResponse, started: std::time::Instant) {
    if !out.is_null() {
        *out = HttpResponse {
            status: resp.status().as_u16() as i32,
            headers: std::ptr::null_mut(),
            header_count: 0,
            body_bytes: std::ptr::null_mut(),
            body_len: 0,
            redirected_url: std::ptr::null_mut(),
            elapsed_ms: started.elapsed().as_millis() as u64,
        };
    }
}

unsafe extern "C" fn request_send(_: *mut c_void, req: *const HttpRequest, out: *mut HttpResponse) -> Res {
    let started = std::time::Instant::now();
    let resp = match request_of(req, 0) {
        Ok(r) => r,
        Err(e) => return e,
    };
    meta(&resp, out, started);
    let body = resp.bytes().map(|b| b.to_vec()).unwrap_or_default();
    if !body.is_empty() {
        let buffer = libc::malloc(body.len()) as *mut u8;
        std::ptr::copy_nonoverlapping(body.as_ptr(), buffer, body.len());
        (*out).body_bytes = buffer;
        (*out).body_len = body.len();
    }
    RAC_SUCCESS
}

unsafe fn stream(req: *const HttpRequest, resume: u64, cb: ChunkFn, user: *mut c_void, out: *mut HttpResponse) -> Res {
    let started = std::time::Instant::now();
    let mut resp = match request_of(req, resume) {
        Ok(r) => r,
        Err(e) => return e,
    };
    meta(&resp, out, started);
    let length = resp.content_length().map(|l| l + resume).unwrap_or(0);
    let mut written = resume;
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        match resp.read(&mut buffer) {
            Ok(0) => return RAC_SUCCESS,
            Ok(n) => {
                written += n as u64;
                if cb(buffer.as_ptr(), n, written, length, user) == 0 {
                    return RAC_ERROR_CANCELLED;
                }
            }
            Err(_) => return RAC_ERROR_NETWORK_ERROR,
        }
    }
}

unsafe extern "C" fn request_stream(
    _: *mut c_void,
    req: *const HttpRequest,
    cb: ChunkFn,
    user: *mut c_void,
    out: *mut HttpResponse,
) -> Res {
    stream(req, 0, cb, user, out)
}

unsafe extern "C" fn request_resume(
    _: *mut c_void,
    req: *const HttpRequest,
    from: u64,
    cb: ChunkFn,
    user: *mut c_void,
    out: *mut HttpResponse,
) -> Res {
    stream(req, from, cb, user, out)
}

static ADAPTER: PlatformAdapter = PlatformAdapter {
    abi_version: RAC_PLATFORM_ADAPTER_ABI_VERSION,
    struct_size: std::mem::size_of::<PlatformAdapter>() as u32,
    file_exists: Some(file_exists),
    file_read: Some(file_read),
    file_write: Some(file_write),
    file_delete: Some(file_delete),
    secure_get: Some(secure_get),
    secure_set: Some(secure_set),
    secure_delete: Some(secure_delete),
    log: Some(log),
    now_ms: Some(now_ms),
    get_memory_info: Some(memory_info),
    http_download: None,
    http_download_cancel: None,
    extract_archive: None,
    file_list_directory: None,
    is_non_empty_directory: Some(non_empty_dir),
    get_vendor_id: None,
    user_data: std::ptr::null_mut(),
};

// SAFETY: immutable after construction; RACommons only reads it.
unsafe impl Sync for PlatformAdapter {}

static TRANSPORT: TransportOps = TransportOps {
    request_send: Some(request_send),
    request_stream: Some(request_stream),
    request_resume: Some(request_resume),
    init: None,
    destroy: None,
};

// ---------------------------------------------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------------------------------------------

/// RACommons, loaded and initialised. Keeps its libraries open for as long as it lives (RACommons is process-global:
/// one per process, never unloaded).
pub struct RunAnywhere {
    api: Api,
    pub version: String,
    _libraries: Vec<Library>,
}

// SAFETY: plain C function pointers; RACommons' components lock internally (documented thread-safe).
unsafe impl Send for RunAnywhere {}
unsafe impl Sync for RunAnywhere {}

impl RunAnywhere {
    /// Opens the package's libraries in order, each hashed against the catalogue right before it is opened (the
    /// same rule as sherpa-onnx), then `rac_init` + the transport + the sherpa backend.
    pub fn load(root: &Path, libraries: &[sidevoice_desktop_core::engines::Library]) -> Result<RunAnywhere, String> {
        let mut opened = Vec::new();
        for library in libraries {
            let path = root.join(&library.path);
            let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let got = crate::install::hex(&<sha2::Sha256 as sha2::Digest>::digest(&bytes));
            if got != library.sha256.to_ascii_lowercase() {
                return Err(format!("{} is not the library the catalog names (SHA-256 {got})", path.display()));
            }
            // SAFETY: loading runs the library's initialisers; these are the pinned, hash-checked package's own.
            opened.push(unsafe { Library::new(&path) }.map_err(|e| format!("cannot load {}: {e}", path.display()))?);
        }
        let api = Api {
            init: symbol(&opened, "rac_init")?,
            error_message: symbol(&opened, "rac_error_message")?,
            error_details: symbol(&opened, "rac_error_get_details")?,
            sdk_version: symbol(&opened, "rac_sdk_get_version")?,
            transport_register: symbol(&opened, "rac_http_transport_register")?,
            download_execute: symbol(&opened, "rac_http_download_execute")?,
            backend_sherpa_register: symbol(&opened, "rac_backend_sherpa_register")?,
            stt_create: symbol(&opened, "rac_stt_component_create")?,
            stt_load: symbol(&opened, "rac_stt_component_load_model")?,
            stt_transcribe: symbol(&opened, "rac_stt_component_transcribe")?,
            stt_result_free: symbol(&opened, "rac_stt_result_free")?,
            stt_unload: symbol(&opened, "rac_stt_component_unload")?,
            stt_destroy: symbol(&opened, "rac_stt_component_destroy")?,
            tts_create: symbol(&opened, "rac_tts_component_create")?,
            tts_load: symbol(&opened, "rac_tts_component_load_voice")?,
            tts_synthesize: symbol(&opened, "rac_tts_component_synthesize")?,
            tts_result_free: symbol(&opened, "rac_tts_result_free")?,
            tts_unload: symbol(&opened, "rac_tts_component_unload")?,
            tts_destroy: symbol(&opened, "rac_tts_component_destroy")?,
        };
        // SAFETY: returns a static string.
        let version = unsafe { CStr::from_ptr((api.sdk_version)()) }.to_string_lossy().into_owned();
        let engine = RunAnywhere { api, version, _libraries: opened };
        let tag = c"sidevoice-spike";
        let config =
            Config { platform_adapter: &ADAPTER, log_level: RAC_LOG_WARNING, log_tag: tag.as_ptr(), reserved: std::ptr::null_mut() };
        // SAFETY: ADAPTER and the tag are 'static, as rac_init requires (kept until rac_shutdown).
        engine.check("rac_init", unsafe { (engine.api.init)(&config) })?;
        engine.check("rac_http_transport_register", unsafe {
            (engine.api.transport_register)(&TRANSPORT, std::ptr::null_mut())
        })?;
        engine.check("rac_backend_sherpa_register", unsafe { (engine.api.backend_sherpa_register)() })?;
        Ok(engine)
    }

    /// `step: message (details)` for a failed call, from RACommons' own error text.
    fn check(&self, step: &str, code: Res) -> Result<(), String> {
        if code == RAC_SUCCESS {
            return Ok(());
        }
        // SAFETY: both return static or thread-local NUL-terminated strings (or NULL).
        let message = path_of(unsafe { (self.api.error_message)(code) }).unwrap_or_default();
        let details = path_of(unsafe { (self.api.error_details)() }).unwrap_or_default();
        Err(format!("{step}: {code} {message} {details}").trim().to_string())
    }

    /// RunAnywhere's own download path (`rac_http_download_execute`): streamed through our transport, SHA-256 checked
    /// inline when `sha256` is given. Returns its status (0 ok, 5 checksum failed, 6 cancelled…) and the HTTP status.
    /// `progress` returns false to cancel.
    pub fn download(&self, url: &str, destination: &Path, sha256: Option<&str>, progress: &mut dyn FnMut(u64, u64) -> bool) -> (i32, i32) {
        unsafe extern "C" fn on_progress(done: u64, total: u64, user: *mut c_void) -> i32 {
            let f = &mut *(user as *mut &mut dyn FnMut(u64, u64) -> bool);
            f(done, total) as i32
        }
        let url = CString::new(url).unwrap();
        let dest = CString::new(destination.to_string_lossy().as_bytes()).unwrap();
        let sha = sha256.map(|s| CString::new(s).unwrap());
        let request = DownloadRequest {
            url: url.as_ptr(),
            destination_path: dest.as_ptr(),
            headers: std::ptr::null(),
            header_count: 0,
            timeout_ms: 0,
            follow_redirects: 1,
            resume_from_byte: 0,
            expected_sha256_hex: sha.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
        };
        let mut http_status = 0;
        let mut f: &mut dyn FnMut(u64, u64) -> bool = progress;
        // SAFETY: every pointer outlives the blocking call.
        let status = unsafe {
            (self.api.download_execute)(&request, Some(on_progress), &mut f as *mut _ as *mut c_void, &mut http_status)
        };
        (status, http_status)
    }
}

/// A loaded RACommons STT component: one model, unloaded and destroyed on drop.
pub struct Recognizer {
    engine: Arc<RunAnywhere>,
    handle: Handle,
    language: CString,
    lock: Mutex<()>,
}

unsafe impl Send for Recognizer {}
unsafe impl Sync for Recognizer {}

impl Recognizer {
    /// `dir`: the model's directory; RACommons' sherpa backend picks the files itself by name (it cannot be told
    /// which: `load_model` takes a directory, and it prefers `*.int8.onnx` when both precisions are present).
    pub fn new(engine: Arc<RunAnywhere>, dir: &Path, id: &str, language: &str) -> Result<Recognizer, String> {
        let mut handle: Handle = std::ptr::null_mut();
        engine.check("rac_stt_component_create", unsafe { (engine.api.stt_create)(&mut handle) })?;
        let path = CString::new(dir.to_string_lossy().as_bytes()).unwrap();
        let id = CString::new(id).unwrap();
        let loaded = unsafe { (engine.api.stt_load)(handle, path.as_ptr(), id.as_ptr(), id.as_ptr()) };
        if let Err(e) = engine.check("rac_stt_component_load_model", loaded) {
            unsafe { (engine.api.stt_destroy)(handle) };
            return Err(e);
        }
        Ok(Recognizer { engine, handle, language: CString::new(language).unwrap(), lock: Mutex::new(()) })
    }

    /// RACommons' component takes 16-bit PCM (or a WAV); our f32 samples are quantised on the way in.
    pub fn transcribe(&self, samples: &[f32], sample_rate: i32) -> Result<String, String> {
        let pcm: Vec<i16> = samples.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
        let options = SttOptions {
            language: self.language.as_ptr(),
            detect_language: self.language.as_bytes().is_empty() as i32,
            enable_punctuation: 1,
            enable_diarization: 0,
            max_speakers: 0,
            enable_timestamps: 0,
            audio_format: RAC_AUDIO_FORMAT_PCM,
            sample_rate,
        };
        let mut result: SttResult = unsafe { std::mem::zeroed() };
        let _guard = self.lock.lock().map_err(|_| "poisoned")?;
        let code = unsafe {
            (self.engine.api.stt_transcribe)(
                self.handle,
                pcm.as_ptr() as *const c_void,
                pcm.len() * 2,
                &options,
                &mut result,
            )
        };
        self.engine.check("rac_stt_component_transcribe", code)?;
        let text = path_of(result.text).unwrap_or_default();
        unsafe { (self.engine.api.stt_result_free)(&mut result) };
        Ok(text.trim().to_string())
    }
}

impl Drop for Recognizer {
    fn drop(&mut self) {
        unsafe {
            (self.engine.api.stt_unload)(self.handle);
            (self.engine.api.stt_destroy)(self.handle);
        }
    }
}

/// A loaded RACommons TTS component (sherpa backend: VITS/Piper only).
pub struct Voice {
    engine: Arc<RunAnywhere>,
    handle: Handle,
    lock: Mutex<()>,
}

unsafe impl Send for Voice {}
unsafe impl Sync for Voice {}

impl Voice {
    pub fn new(engine: Arc<RunAnywhere>, dir: &Path, id: &str) -> Result<Voice, String> {
        let mut handle: Handle = std::ptr::null_mut();
        engine.check("rac_tts_component_create", unsafe { (engine.api.tts_create)(&mut handle) })?;
        let path = CString::new(dir.to_string_lossy().as_bytes()).unwrap();
        let id = CString::new(id).unwrap();
        let loaded = unsafe { (engine.api.tts_load)(handle, path.as_ptr(), id.as_ptr(), id.as_ptr()) };
        if let Err(e) = engine.check("rac_tts_component_load_voice", loaded) {
            unsafe { (engine.api.tts_destroy)(handle) };
            return Err(e);
        }
        Ok(Voice { engine, handle, lock: Mutex::new(()) })
    }

    /// The speaker id travels as the voice string (RACommons' sherpa backend `stoi`s it).
    pub fn synthesize(&self, text: &str, sid: i32, speed: f32) -> Result<Audio, String> {
        let text = CString::new(text).map_err(|_| "NUL in text")?;
        let voice = CString::new(sid.to_string()).unwrap();
        let options = TtsOptions {
            voice: voice.as_ptr(),
            language: std::ptr::null(),
            rate: speed,
            pitch: 1.0,
            volume: 1.0,
            audio_format: RAC_AUDIO_FORMAT_PCM,
            sample_rate: 0,
            use_ssml: 0,
        };
        let mut result: TtsResult = unsafe { std::mem::zeroed() };
        let _guard = self.lock.lock().map_err(|_| "poisoned")?;
        let code = unsafe { (self.engine.api.tts_synthesize)(self.handle, text.as_ptr(), &options, &mut result) };
        self.engine.check("rac_tts_component_synthesize", code)?;
        // The sherpa backend hands back f32 PCM; the component passes it through as PCM bytes.
        let samples = if result.audio_data.is_null() {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(result.audio_data as *const f32, result.audio_size / 4) }.to_vec()
        };
        let sample_rate = result.sample_rate;
        unsafe { (self.engine.api.tts_result_free)(&mut result) };
        Ok(Audio { samples, sample_rate })
    }
}

impl Drop for Voice {
    fn drop(&mut self) {
        unsafe {
            (self.engine.api.tts_unload)(self.handle);
            (self.engine.api.tts_destroy)(self.handle);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Our engine contract (crate::Runtime / Adapter) on top
// ---------------------------------------------------------------------------------------------------------------

pub struct Runtime(pub Arc<RunAnywhere>);

/// RACommons is process-global (`rac_init` once): the one instance, once loaded.
static LOADED: OnceLock<Arc<RunAnywhere>> = OnceLock::new();

pub fn instance() -> Option<Arc<RunAnywhere>> {
    LOADED.get().cloned()
}

/// `Adapter::load`: RACommons from the root of its unpacked package.
pub fn load(root: &Path, package: &Package) -> Result<Arc<dyn crate::Runtime>, crate::Error> {
    if let Some(engine) = LOADED.get() {
        return Ok(Arc::new(Runtime(engine.clone())));
    }
    let engine = RunAnywhere::load(root, &package.libraries).map_err(|e| error::runtime_failed(ENGINE, e))?;
    let engine = LOADED.get_or_init(|| Arc::new(engine)).clone();
    Ok(Arc::new(Runtime(engine)))
}

/// `Adapter::files`: RACommons cannot be told which files to use, so the build's directory must hold them under
/// the names its scanner looks for; the catalogue config is only used to check they are there.
pub fn files(family: &str, config: &serde_json::Value) -> Result<Vec<String>, crate::Error> {
    let names = |keys: &[&str]| {
        keys.iter().filter_map(|k| config.get(*k).and_then(|v| v.as_str()).map(String::from)).collect::<Vec<_>>()
    };
    match family {
        "whisper" => Ok(names(&["encoder", "decoder", "tokens"])),
        "vits" => Ok(names(&["model", "tokens", "data_dir"])),
        other => Err(error::family_unsupported(ENGINE, other)),
    }
}

impl crate::Runtime for Runtime {
    fn recognizer(
        &self,
        family: &str,
        dir: &Path,
        _config: &serde_json::Value,
        language: &str,
        accelerator: Capability,
    ) -> Result<Box<dyn Recognize>, crate::Error> {
        if family != "whisper" {
            return Err(error::family_unsupported(ENGINE, family));
        }
        if accelerator != Capability::Cpu {
            return Err(error::runtime_failed(ENGINE, "RACommons' public Linux build is CPU only"));
        }
        let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let r = Recognizer::new(self.0.clone(), dir, &id, language).map_err(|e| error::runtime_failed(ENGINE, e))?;
        Ok(Box::new(r))
    }

    fn voice(
        &self,
        family: &str,
        dir: &Path,
        _config: &serde_json::Value,
        _language: &str,
        accelerator: Capability,
    ) -> Result<Box<dyn Speak>, crate::Error> {
        // RACommons v0.20.37's sherpa backend builds only `model.vits` (Piper): Kokoro is NeuRT (closed, Apple) or
        // QHexRT (Snapdragon) there, never on a desktop CPU.
        if family != "vits" {
            return Err(error::family_unsupported(ENGINE, family));
        }
        if accelerator != Capability::Cpu {
            return Err(error::runtime_failed(ENGINE, "RACommons' public Linux build is CPU only"));
        }
        let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let v = Voice::new(self.0.clone(), dir, &id).map_err(|e| error::runtime_failed(ENGINE, e))?;
        Ok(Box::new(v))
    }
}

impl Recognize for Recognizer {
    fn transcribe(&self, samples: &[f32], sample_rate: i32) -> Result<String, crate::Error> {
        Recognizer::transcribe(self, samples, sample_rate).map_err(|e| error::runtime_failed(ENGINE, e))
    }
}

impl Speak for Voice {
    fn synthesize(&self, text: &str, sid: i32, speed: f32) -> Result<Audio, crate::Error> {
        Voice::synthesize(self, text, sid, speed).map_err(|e| error::runtime_failed(ENGINE, e))
    }
}
