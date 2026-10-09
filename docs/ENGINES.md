# The native engine: sidevoice-engine

The app's local speech models run on **sidevoice-engine** (`sidevoice/sidevoice-engine`), a Rust crate this app
depends on (`src-tauri/engine/Cargo.toml`, pinned to one commit until the engine is tagged). The engine owns the
catalogue of local models, the downloads, which build of a model runs on this machine, and the models in memory. This
app owns what is the app's: the bridge the page calls (docs/BRIDGE.md → "The native engine"), its install jobs, its
refusals, and what stays in memory and for how long (sidevoice/sidevoice-core#21 D13). That side is
`src-tauri/engine/` (`sidevoice-desktop-engine`):

- `lib.rs`: `NativeEngines`, the page's calls in the page's terms over `sidevoice_engine::Engine`.
- `residency.rs`: the models in memory, one per (engine, model, accelerator), and D13. The engine unloads a model when
  the last `LoadedModel` of it is dropped; the app holds them here.
- `jobs.rs`: each `install` call's progress and cancel.
- `error.rs`: the keyed refusals, and how each of the engine's codes becomes one (with the code beside it as `code`).
- `memory.rs`: what is available of the machine's memory now.

## The catalogue

**Owner: sidevoice-engine** (operator's decision, 2026-10-09). The app runs the engine's bundled catalogue
(`BundledCatalog`, `catalog/families/` in that repository, at the commit `src-tauri/Cargo.lock` pins) and has none of
its own.

The page still resolves its offers from the catalogue it carries, and sidevoice-core still validates a device stage
against its own copy, until the core takes the engine's (a sidevoice-core issue). So the ids the page sends must be
the engine's too. They are, for every native build the core's catalogue lists today:

- models `whisper-tiny`, `whisper-base`, `whisper-small`, `whisper-large-v3-turbo`, `kokoro-82m-v1.0`;
- engine `sherpa-onnx` (the engine's backend id); the engine's `whisper-cpp` builds of Whisper are not in the core's
  catalogue, so the page does not offer them yet, though the app's engine commands run them (below);
- Kokoro's voices, all 17 the core lists (`ef_dora`, `em_alex`, `em_santa`, `af_heart`, …).

Where the two catalogues differ (to be resolved when the core takes the engine's):

- **Accelerators.** The app offers no Core ML: the engine's sherpa-onnx builds run on the CPU, its whisper.cpp builds
  on Metal on Apple Silicon. The app reports `has` from them (`["cpu", "metal"]` there), so the page offers the CPU
  for the core's sherpa-onnx builds; a choice of Core ML is refused (`accelerator_unusable`).
- **Files and sizes.** The core's Whisper builds are sherpa-onnx's release archives; the engine's are the same int8
  models as separate files from Hugging Face, pinned by revision. The download size the page shows before asking
  comes from the core's catalogue; the progress the app reports comes from the engine's.
- **Voice languages** are spelled as BCP 47 in the engine (`en-US`, `fr`, `pt-BR`; the core: `en-us`, `fr-fr`,
  `pt-br`). The app reads a voice's language from the engine's catalogue; the page never sends one for a voice.
- The engine lists more models than the core (`whisper-large-v3`, Kokoro v0.19, Parakeet…): the page never asks for
  them, since it offers from the core's.

## What runs, where

- Backends, both linked statically into the app through sidevoice-engine `v0.2.0` (the tag `src-tauri/engine/Cargo.toml`
  pins), with no Cargo features to choose: **sherpa-onnx 1.13.8** (ONNX Runtime) on the CPU, and **whisper.cpp**
  (through `whisper-rs`), compiled from source, on Metal on Apple Silicon. Nothing is loaded at run time with
  `dlopen`, so the macOS app keeps library validation on: its only entitlement is the microphone.
- Models are downloaded on demand into the app's data directory, `sidevoice-engine/`
  (`~/Library/Application Support/dev.sidevoice.desktop/sidevoice-engine/` on macOS), each file checked against its
  SHA-256 as it arrives and stored only once whole.
- Whisper for speech to text, Kokoro for speech; one model in memory serves every language. The language of a
  transcription goes to the engine with the call (`x-language`; empty: Whisper detects it). A voice's language goes
  with each synthesis.
- Accelerator: the one the engine runs the build on here (the CPU for sherpa-onnx, Metal for whisper.cpp on Apple
  Silicon), which is what the page is offered.

## Building

The engine's sherpa-onnx backend links sherpa-onnx's prebuilt static libraries. CI fetches them with the engine's own
`cargo xtask sherpa-libs`, at the pinned commit, checked against the digests it pins, and links them through
`SHERPA_ONNX_LIB_DIR` (`.github/actions/setup-rust`). Locally, from a checkout of sidevoice-engine at that commit:

    export SHERPA_ONNX_LIB_DIR="$(cargo xtask sherpa-libs)"

Without it, sherpa-onnx's build script downloads them itself, unchecked. The engine's whisper.cpp backend compiles
whisper.cpp and ggml from source: the build needs CMake, a C++ compiler and libclang (for `bindgen`); on Linux
x86_64, `src-tauri/.cargo/config.toml` compiles it with libstdc++'s old string ABI, as sherpa-onnx's libraries there
are. The Rust version is the engine's, `src-tauri/rust-toolchain.toml`.

## Verified in CI

The job "Native engine on Apple Silicon" runs `src-tauri/engine/examples/roundtrip.rs`: download Kokoro and Whisper
tiny, load both, Kokoro says a sentence in Spanish, English and Spanish again and Whisper hears each, through the one
model each has in memory: on the CPU, naming it, and with Whisper on whisper.cpp, which must run on Metal. The macOS
job's probe page and room flow run the same through the signed app's IPC.

## Changing the engine

Moving the pin is one line, the `tag` in `src-tauri/engine/Cargo.toml`, then `cargo update -p sidevoice-engine`. A model
or a voice is added in sidevoice-engine's catalogue, never here; the page offers it once the core's catalogue (for now)
has it too.
