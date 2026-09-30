# Model engines: one catalog, chosen at run time

Operator's direction (2026-09-30): engines are **downloadable packages chosen at run time**, not compiled into
a build. One catalog serves every client (browser, desktop app, node, later mobile). Each client detects what it
can do and offers only the engines and models that fit; picking one downloads the engine (if missing) and the
model. The experience is the same wherever it runs. Bring-your-own-model later = one more model entry in a format
some engine reads.

## The catalog (`catalog/engines.json`, shape in `src-tauri/core/src/engines.rs`)

```jsonc
{
  "version": 1,
  "engines": [
    { "id": "sherpa-onnx", "version": "1.13.8", "runs": "native",      // a package per OS/arch
      "formats": ["onnx"], "tasks": ["stt", "tts"],
      "packages": [{ "os": "macos", "arch": "aarch64",
                     "requires": [],                                  // e.g. ["cuda"] for a CUDA build
                     "accelerators": ["coreml", "cpu"],               // what it can use, best first
                     "download": { "url", "sha256", "size", "archive": "tar.bz2", "root" },
                     "libraries": ["lib/libonnxruntime.dylib", "lib/libsherpa-onnx-c-api.dylib"] }] },
    { "id": "transformers-js", "runs": "browser", "needs_any": ["wasm", "webgpu"], … }
  ],
  "models": [
    { "id": "whisper-tiny", "task": "stt", "languages": ["multi"],
      "builds": {                                                     // per engine it runs on
        "sherpa-onnx":     { "format": "onnx", "download": {…sha256…}, "config": { "kind": "whisper", "encoder", "decoder", "tokens" } },
        "transformers-js": { "format": "onnx", "requires": [],       "config": { "repository": "onnx-community/whisper-tiny", "revision", "dtype" } } } },
    { "id": "kokoro-82m-v1.0", "task": "tts", "voices": [{ "id": "ef_dora", "sid": 28, "language": "es" }, …], … }
  ]
}
```

Capabilities (`Capability`): `cpu`, `wasm`, `webgpu`, `webgpu-f16`, `metal`, `coreml`, `mlx`, `cuda`.
- A native client knows its OS/arch and what they imply (`native_device()`: an Apple Silicon Mac has `cpu`,
  `coreml`, `metal`, `mlx`).
- A page knows `wasm`/`webgpu`/`webgpu-f16` (the web's `stt-engine.js` `capabilities()` already measures them).
- `offers(catalog, device)` = every (model, engine) whose engine fits the device (a native package for its
  OS/arch whose `requires` it has; or a browser engine for which it has one of `needs_any`) and whose build's
  `requires` it has. A desktop app merges the page's capabilities with its own; a browser has only the page's.

`check(catalog)` refuses a catalog with an unknown engine, a format the engine does not read, or a native download
without https + SHA-256 (tested against the bundled one).

## Where the shared catalog should live

**Owner: `sidevoice/sidevoice-core`**, as package data next to the voice catalogue it already owns (LOCAL_MODE_PLAN
L3), served by every node at `GET /api/models/catalog` (and through a room's relay like any node route). Every
client reads it from the node it is paired with; the desktop app keeps its bundled copy only as the offline
fallback, and the web's hard-coded list (`packages/browser-audio/stt-engine.js` `MODELS`, `engine.js` Kokoro
revision) is generated from it. One writer, one copy.

Tonight it lives here (`catalog/engines.json`), because the node endpoint does not exist yet and moving it is a
cross-repo change of its own. This is the **one temporary second copy**: the transformers.js builds in it
repeat the revisions in `stt-engine.js` / `engine.js`. Moving it is the next step (an open decision in
HANDOVER.md).

## The native engine shipped tonight: sherpa-onnx on Apple Silicon

- One runtime for both tasks: **sherpa-onnx 1.13.8** (ONNX Runtime underneath), Whisper for STT (tiny, base,
  small, large-v3-turbo; int8) and Kokoro 82M v1.0 multi-language for TTS (int8; Spanish voices `ef_dora`,
  `em_alex`, `em_santa`, plus English, French, Italian, Portuguese, Hindi — the same voice ids the web uses).
- Downloaded on demand into `~/Library/Application Support/dev.sidevoice.desktop/engines/` (the package 8.8 MB,
  Whisper tiny 116 MB … turbo 564 MB, Kokoro 132 MB), each checked against the catalog's SHA-256, unpacked into a
  temporary directory and moved into place only when complete (`src-tauri/engine/src/install.rs`).
- Loaded with `dlopen` (`libloading`), never linked: ONNX Runtime first, then the C API. The `#[repr(C)]`
  structs are **generated** from that exact version's `c-api.h` (`scripts/gen-sherpa-ffi.py`), and the library's
  own version string is checked against the bindings' before anything is called.
- Provider: **CPU** by default. Core ML was measured slower on the CI runner for these models (Kokoro 3.1 s vs
  2.4 s for 2.7 s of speech; Whisper tiny 2.0 s vs 0.3 s); `SIDEVOICE_ENGINE_PROVIDER=coreml` switches it.
- macOS: the app is hardened-runtime, ad-hoc signed; library validation would refuse a downloaded library, so
  the app carries `com.apple.security.cs.disable-library-validation` (Entitlements.plist explains the trade).

Verified on a real Apple Silicon runner (CI job "Native engine"): download both, Kokoro says
"Hola, esto es una prueba de voz de Sidevoice." with `ef_dora`, Whisper tiny hears
"Hola, esto es una prueba de voz de si de voice." (0.29 s). The macOS job's probe runs the same through the
signed app's IPC.

## The page's side

`window.__sidevoiceDesktop.host.nativeEngine` (bridge/desktop-bridge.js):

| Call | Does |
|---|---|
| `available()` | `{device, offers:[{model, engine, task, label, languages, downloadSize, accelerators, installed, voices}], pageIds}` — `pageIds` maps the page's model ids (`onnx-community/whisper-small`) to the catalog's (`whisper-small`) |
| `install(model, engine, onProgress)` | downloads engine + model; `onProgress(doneBytes, totalBytes)` |
| `transcribe(model, Float32Array, sampleRate, language)` | text |
| `synthesize(model, voice, speed, text)` | `{samples: Float32Array, sampleRate}` |

In the web interface the native engine is one more **device** next to WebGPU and WebAssembly for the same model:
"where does this model run". The web's transcription and voice clients talk to their engines through a worker
message protocol; with the native device chosen they get a stand-in with the same protocol backed by
`nativeEngine` (rubasace/sidevoice `packages/browser-audio/native-worker.js`).

## Adding an engine or a model

- A model in a format an engine already reads: one entry in `models`, with a `builds.<engine>` carrying its
  download (native) or its repository (browser) and the engine's config keys. Nothing is compiled.
- A native engine: a catalog entry with packages per OS/arch, and — the one compiled part — its runtime adapter
  in `src-tauri/engine/src/` (load the package, map the model's config to the engine's API). An engine whose
  adapter the app does not have is simply not offered.
