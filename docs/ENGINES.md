# Model engines: one catalog, chosen at run time

Engines are **downloadable packages chosen at run time**, not compiled into
a build. One catalog serves every client (browser, desktop app, node, later mobile). Each client detects what it
can do and offers only the engines and models that fit; picking one downloads the engine (if missing) and the
model. The experience is the same wherever it runs. Bring-your-own-model later = one more model entry in a format
some engine reads.

## The catalog (`catalog/engines.json`, shape in `src-tauri/core/src/engines.rs`)

**Owner: `sidevoice/sidevoice-core`** (`src/sidevoice_core/models/catalog.json`), as sidevoice/sidevoice-core#21 §3
decided. `catalog/engines.json` is a copy generated from it and never edited here. To change it, change the core,
then:

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
(`webgpu-f16`), and may carry a per-platform `rank` that beats the default engine order (the resolvers' concern).

Capabilities (`Capability`): `cpu`, `wasm`, `webgpu`, `webgpu-f16`, `metal`, `coreml`, `mlx`, `cuda`; one a newer
catalog names and this app does not know is never had. A `Device` is what a place reports: `runs` (`page` or
`native`), OS/arch, what it `has`, and memory when known. This app's is `native_device()` (an Apple Silicon Mac has
`cpu`, `coreml`, `metal`, `mlx`) with the machine's total memory from the OS (`src-tauri/engine/src/memory.rs`); it is
what the page gets from `nativeEngine.capabilities()` (docs/BRIDGE.md).

The resolver of sidevoice-core#21 — one offer per model a place can run, on its best build and accelerator — has one
implementation per side: the web's TypeScript for a client (in the app too: the page resolves from what
`nativeEngine.capabilities()` reports) and the core's Python for a host. This app has none. At its trust boundary
it only checks that the build a page asks for runs here before it downloads or runs it (`src-tauri/engine`,
`locate`): an adapter for the engine, a package for this device, the build's needs and accelerators
(`accelerators_for` in `src-tauri/core/src/engines.rs`) and the model's `requires.memory_mb` (unknown memory is not
a refusal). The accelerator the page names must be one of those; with none named, the first is used.

`check(catalog)` refuses a catalog with an unknown engine or family, a build its engine does not run (family,
format, accelerator), a native download without https + SHA-256, a library without its hash, or an option kind the
web interface cannot render (tested against the bundled catalog).

## The native engine shipped tonight: sherpa-onnx on Apple Silicon

- One runtime for both tasks: **sherpa-onnx 1.13.8** (ONNX Runtime underneath), Whisper for STT (tiny, base,
  small, large-v3-turbo; int8) and Kokoro 82M v1.0 multi-language for TTS (int8; Spanish voices `ef_dora`,
  `em_alex`, `em_santa`, plus English, French, Italian, Portuguese, Hindi — the same voice ids the web uses).
- Downloaded on demand into `~/Library/Application Support/dev.sidevoice.desktop/engines/` (the package 8.8 MB,
  Whisper tiny 116 MB … turbo 564 MB, Kokoro 132 MB), each checked against the catalog's SHA-256, unpacked into a
  temporary directory and moved into place only when complete (`src-tauri/engine/src/install.rs`). A download counts
  as installed while its marker names its hash and its root and every file the engine needs are there (the
  package's libraries; the files the model's config names); one that lost a file is not, and installing fetches it
  again. A download that fails or is cancelled (`cancel(job)`, docs/BRIDGE.md) leaves nothing of itself on disk.
- Loaded with `dlopen` (`libloading`), never linked: ONNX Runtime first, then the C API. The `#[repr(C)]`
  structs are **generated** from that exact version's `c-api.h` (`scripts/gen-sherpa-ffi.py`), and the library's
  own version string is checked against the bindings' before anything is called.
- One model in memory serves every language: Whisper's is switched per call on the loaded recognizer
  (`SherpaOnnxOfflineRecognizerSetConfig`, which only replaces its Whisper settings) and Kokoro's per synthesis
  (`extra.lang`). What is loaded, and for how long, is docs/BRIDGE.md → "What stays in memory".
- Accelerator: the one the page chose for the build, else the first it can use here — **CPU**, which the catalogue
  ranks first for sherpa-onnx on Apple Silicon: Core ML was measured slower on the CI runner for these models
  (Kokoro 3.1 s vs
  2.4 s for 2.7 s of speech; Whisper tiny 2.0 s vs 0.3 s). The CI round trip also runs it with `coreml`.
- macOS: the app is hardened-runtime, ad-hoc signed; library validation would refuse a downloaded library, so
  the app carries `com.apple.security.cs.disable-library-validation` (Entitlements.plist explains the trade).

Verified on a real Apple Silicon runner (CI job "Native engine"): download both, Kokoro says
"Hola, esto es una prueba de voz de Sidevoice." with `ef_dora`, Whisper tiny hears
"Hola, esto es una prueba de voz de si de voice." (0.29 s). The macOS job's probe runs the same through the
signed app's IPC.

## The page's side

The page reaches the engine through `window.__sidevoiceDesktop.host.nativeEngine` — `capabilities()`,
`installed()`, `install(model, engine, onProgress)`, `transcribe(…)`, `synthesize(…)`, keyed by catalogue model id +
engine id; the contract is in docs/BRIDGE.md → "The native engine". In the app the native engine is the only
engine *Este dispositivo* has: the page's own engines (WebGPU, WebAssembly) are never offered there (sidevoice-core#21 D5). The
web's transcription and voice clients talk to their engines through a worker message protocol; for a native offer
they get a stand-in with the same protocol backed by `nativeEngine` (sidevoice-web
`packages/browser-audio/native-worker.js`).

The app's settings window shows the engine's side: what `capabilities()` reports, and the engine packages and model
builds on disk with their sizes.

## Adding an engine or a model

- A model of a family an engine already runs: one entry in the core's `models`, with a build per engine carrying
  its download (native) or its repository (page) and the engine's config keys; then copy the catalog here. Nothing
  is compiled.
- A native engine: a catalog entry in the core with packages per OS/arch, and — the one compiled part — its runtime adapter
  in `src-tauri/engine/src/` (a `Runtime`: load the package, map the model's config to the engine's API) and a line in
  `ADAPTERS`. The app refuses to install or run a build on an engine it has no adapter for; it never substitutes
  another. The page resolves offers from the capabilities alone, which do not say which engines the app runs: a
  catalogue with a second native engine for this platform and no adapter here would be offered, then refused.
