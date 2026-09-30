# Model engines: one catalog, chosen at run time

Operator's direction (2026-09-30): engines are **downloadable packages chosen at run time**, not compiled into
a build. One catalog serves every client (browser, desktop app, node, later mobile). Each client detects what it
can do and offers only the engines and models that fit; picking one downloads the engine (if missing) and the
model. The experience is the same wherever it runs. Bring-your-own-model later = one more model entry in a format
some engine reads.

## The catalog (`catalog/engines.json`, shape in `src-tauri/core/src/engines.rs`)

**Owner: `sidevoice/sidevoice-core`** (`src/sidevoice_core/models/catalog.json`), as rubasace/sidevoice#124 §3
decided. `catalog/engines.json` is a copy generated from it and never edited here; so is `catalog/vectors.json`,
the resolver's shared test vectors. To change either, change the core, then:

    node scripts/copy-core-catalog.mjs <sidevoice-core checkout>

Every node serves the same file at `GET /api/models/catalog`; the web build carries its own generated copy.

```jsonc
{
  "version": 2,
  "engines": [
    { "id": "sherpa-onnx", "version": "1.13.8", "runs": "native",     // a package per OS/arch
      "families": ["whisper", "kokoro"], "formats": ["onnx"],
      "packages": [{ "os": "macos", "arch": "aarch64",
                     "requires": [],                                 // e.g. ["cuda"] for a CUDA build
                     "accelerators": ["cpu", "coreml"],              // best first: Core ML measured slower
                     "download": { "url", "sha256", "size", "archive": "tar.bz2", "root" },
                     "libraries": [{ "path": "lib/libonnxruntime.dylib", "sha256" }, …] }] },
    { "id": "transformers-js", "runs": "page", "accelerators": ["webgpu", "wasm"], … }
  ],
  "ranking": { "default": ["sherpa-onnx", "transformers-js"] },
  "families": { "whisper": { "task": "stt", "options": [ … ] }, "kokoro": { "task": "tts", "options": [ … ] } },
  "providers": [ { "id": "openai", "tasks": ["stt"], … }, { "id": "elevenlabs", "tasks": ["tts"], … } ],
  "models": [
    { "id": "whisper-tiny", "family": "whisper", "languages": ["multi"],
      "builds": [                                                    // one per engine it runs on
        { "engine": "sherpa-onnx", "format": "onnx", "download": {…sha256…}, "config": { "encoder", "decoder", "tokens" } },
        { "engine": "transformers-js", "format": "onnx", "config": { "repository": "onnx-community/whisper-tiny", "revision", "dtype" } } ] },
    { "id": "kokoro-82m-v1.0", "family": "kokoro", "voices": [{ "id": "ef_dora", "language": "es", "sid": 28 }, …], … }
  ]
}
```

A build may be limited to some `accelerators` (Whisper small's page build: WebGPU only), may `need` a feature
(`webgpu-f16`), and may carry a per-platform `rank` that beats the default engine order.

Capabilities (`Capability`): `cpu`, `wasm`, `webgpu`, `webgpu-f16`, `metal`, `coreml`, `mlx`, `cuda`; one a newer
catalog names and this app does not know is never had. A `Device` is what a place reports: `runs` (`page` or
`native`), OS/arch, what it `has`, and memory when known. This app's is `native_device()` (an Apple Silicon Mac has
`cpu`, `coreml`, `metal`, `mlx`).

`offers(catalog, device, place)` is the resolver of #124 §4: one offer per model the place can run, on its best
build (the build's `rank` for this platform, else the default order) and best accelerator, with the reason and every
other build × accelerator ranked behind it. A native runtime is never offered a page engine's builds, and a page
never a native one. The core's Python is the reference and the web's TypeScript the third implementation; all
three pass `catalog/vectors.json`.

`check(catalog)` refuses a catalog with an unknown engine or family, a build its engine does not run (family,
format, accelerator), a native download without https + SHA-256, a library without its hash, or an option kind the
web interface cannot render (tested against the bundled one and the vectors' fixture).

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
| `available()` | `{device, offers:[{model, engine, task, label, languages, downloadSize, accelerator, accelerators, reason, installed, voices}], pageIds}` — one offer per model, on its best build; `pageIds` maps the page's model ids (`onnx-community/whisper-small`) to the catalog's (`whisper-small`) |
| `install(model, engine, onProgress)` | downloads engine + model; `onProgress(doneBytes, totalBytes)` |
| `transcribe(model, Float32Array, sampleRate, language)` | text |
| `synthesize(model, voice, speed, text)` | `{samples: Float32Array, sampleRate}` |

In the web interface the native engine is one more **device** next to WebGPU and WebAssembly for the same model:
"where does this model run". The web's transcription and voice clients talk to their engines through a worker
message protocol; with the native device chosen they get a stand-in with the same protocol backed by
`nativeEngine` (rubasace/sidevoice `packages/browser-audio/native-worker.js`).

## Adding an engine or a model

- A model of a family an engine already runs: one entry in the core's `models`, with a build per engine carrying
  its download (native) or its repository (page) and the engine's config keys; then copy the catalog here. Nothing
  is compiled.
- A native engine: a catalog entry in the core with packages per OS/arch, and — the one compiled part — its runtime adapter
  in `src-tauri/engine/src/` (load the package, map the model's config to the engine's API). An engine whose
  adapter the app does not have is simply not offered.
