# The native engine: sidevoice-engine, and the voice call on it

The app's speech models run on **sidevoice-engine** (`sidevoice/sidevoice-engine`), a Rust crate this app depends on
(`src-tauri/engine/Cargo.toml`, pinned to one commit until the engine is tagged). The engine owns the catalogue of
models, local and remote, the downloads, which build of a model runs on this machine, and the models in memory.

On macOS the app runs the voice call itself with **sidevoice-voice** (`sidevoice/sidevoice-voice`, `src-tauri/src/voice.rs`,
docs/BRIDGE.md → "The voice call"), on that same engine: the microphone, voice activity and turns, transcription,
speech, playback, barge-in and what was heard. What is in memory is what the call holds: its three models (the voice
activity detector, speech to text, text to speech) stay loaded across calls, and leave memory once the call has been
stopped for `idle_unload_minutes` (10 by default; a setting of `host.voice`); the next start loads them again. A
change of settings loads the new ones.

This app keeps what is the app's, in `src-tauri/engine/` (`sidevoice-desktop-engine`):

- `lib.rs`: `NativeEngines`, the page's model management in the page's terms over `sidevoice_engine::Engine`, and the
  engine itself for the voice call (`NativeEngines::engine`).
- `jobs.rs`: each `install` call's progress and cancel.
- `error.rs`: the keyed refusals, and how each of the engine's codes becomes one (with the code beside it as `code`).
- `memory.rs`: what is available of the machine's memory now.

## The catalogue

**Owner: sidevoice-engine** (operator's decision, 2026-10-09). The app runs the engine's bundled catalogue
(`BundledCatalog`, `catalog/families/` in that repository, at the commit `src-tauri/Cargo.lock` pins) and has none of
its own. The page lists it through `host.voice.models()`, in the shape `WebEngine.models()` answers on the web.

## What runs, where

- Backends, linked statically into the app through sidevoice-engine (the commit `src-tauri/engine/Cargo.toml` pins),
  with no Cargo features to choose: **sherpa-onnx 1.13.8** (ONNX Runtime) on the CPU, **whisper.cpp** (through
  `whisper-rs`), compiled from source, on Metal on Apple Silicon, and the **remote** OpenAI and ElevenLabs backends.
  Nothing is loaded at run time with `dlopen`, so the macOS app keeps library validation on: its only entitlement is
  the microphone.
- The voice call picks each stage's build (core `voice.rs`): Whisper on whisper.cpp where the model has that build
  (Metal on Apple Silicon), every other model on the build the engine recommends, never Core ML nor MLX, and Silero
  for voice activity, on sherpa-onnx.
- Models are downloaded on demand into the app's data directory, `sidevoice-engine/`
  (`~/Library/Application Support/dev.sidevoice.desktop/sidevoice-engine/` on macOS), each file checked against its
  SHA-256 as it arrives and stored only once whole. The voice call installs what it needs as it starts; the page may
  install a model before (`nativeEngine.install`), to show its progress.
- **Remote models** need their provider's key, which the app keeps in the macOS keychain (`src-tauri/src/keychain.rs`,
  service `dev.sidevoice.desktop.providers`). The engine asks the app for it each time it needs it
  (`NativeHost::with_credentials`); the page sets and clears it (`host.voice.setProviderKey`) and never reads it back.
  Elsewhere the app keeps no keys.
- **Echo cancellation** is the call's own: WebRTC's AEC3, fed with the call's playback (sidevoice-voice's README). The
  room window's page is never granted the microphone where the app runs the call.

## Building

The engine's sherpa-onnx backend links sherpa-onnx's prebuilt static libraries. CI fetches them with the engine's own
`cargo xtask sherpa-libs`, at the pinned commit, checked against the digests it pins, and links them through
`SHERPA_ONNX_LIB_DIR` (`.github/actions/setup-rust`). Locally, from a checkout of sidevoice-engine at that commit:

    export SHERPA_ONNX_LIB_DIR="$(cargo xtask sherpa-libs)"

Without it, sherpa-onnx's build script downloads them itself, unchecked. The engine's whisper.cpp backend compiles
whisper.cpp and ggml from source: the build needs CMake, a C++ compiler and libclang (for `bindgen`); on Linux
x86_64, `src-tauri/.cargo/config.toml` compiles it with libstdc++'s old string ABI, as sherpa-onnx's libraries there
are. On macOS, sidevoice-voice's echo canceller builds WebRTC's C++ with meson and ninja (`brew install meson ninja
pkg-config`). The Rust version is the engine's, `src-tauri/rust-toolchain.toml`.

## Verified in CI

The macOS job's probe page, in the signed app: the engine's capabilities, installs with progress, refusals by key, a
cancelled download; and the voice call's seam, its catalogue, Whisper configured on whisper.cpp, and a start that
settles (listening, or refused with the call's code where the runner has no microphone or speaker). The call itself,
with real models on recorded speech, is sidevoice-voice's own CI.

## Changing the engine

The app links one engine: `src-tauri/engine/Cargo.toml` and sidevoice-voice must pin the same commit (a second copy
would link sherpa-onnx and whisper.cpp twice). Move both, then `cargo update -p sidevoice-engine -p sidevoice-voice`.
A model or a voice is added in sidevoice-engine's catalogue, never here.
