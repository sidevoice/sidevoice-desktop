# Third-party components

Sidevoice's Apache-2.0 license applies to its own code. The components below keep their own licenses. This
repository does not commit model weights or the generated WebAssembly runtimes; the installers do contain some of
them, as listed.

## Inside the installers

- **eSpeak NG** (`espeak-ng` 1.0.2 on npm: its JavaScript and WebAssembly build), **GPL-3.0-or-later**. It is copied
  into `voice-browser/assets/` at build time (`scripts/web-assets.mjs`) and runs in the bundled interface's speech
  workers. Redistributing the installers redistributes it, under the GPL: its license text and source are at
  https://github.com/espeak-ng/espeak-ng, and the build used is the npm package of that version.
- **ONNX Runtime Web** (`onnxruntime-web`, the WebAssembly runtime and its loaders), MIT.
- **Transformers.js** (bundled into the interface's workers), Apache-2.0.
- **The Sidevoice web interface** (sidevoice-web, vendored in `ui/voice/` and `ui/voice-browser/`), Apache-2.0, with
  its own dependencies: React (MIT), the OpenTelemetry JavaScript SDK (Apache-2.0) and the components listed in
  sidevoice-web's `THIRD_PARTY_NOTICES.md`.
- **Tauri** and its plugins (`tauri`, `tauri-plugin-global-shortcut`, `tauri-plugin-single-instance`), MIT or
  Apache-2.0, and the Rust crates in `src-tauri/Cargo.lock` under their own licenses (mostly MIT or Apache-2.0).
- **DM Sans**, the brand typeface (`brand/fonts/`, `ui/brand/`), SIL Open Font License 1.1 (`OFL.txt` beside it).

- **sidevoice-engine** (the native speech engine, a Rust dependency, Apache-2.0) and, linked into the app through it,
  **sherpa-onnx** with **ONNX Runtime** (Apache-2.0 and MIT).

## Downloaded when you first choose a model, not shipped

- **Whisper** models (MIT) and **Kokoro-82M** (Apache-2.0), from the URLs and with the licenses named in
  sidevoice-engine's catalogue (`catalog/families/` in that repository, at the version `src-tauri/Cargo.lock` pins).

Review the upstream licenses and model cards before redistributing compiled assets or weights; this repository's
license does not replace those terms.

- https://github.com/espeak-ng/espeak-ng
- https://github.com/microsoft/onnxruntime
- https://github.com/huggingface/transformers.js
- https://github.com/tauri-apps/tauri
- https://github.com/k2-fsa/sherpa-onnx
- https://huggingface.co/hexgrad/Kokoro-82M
- https://github.com/openai/whisper
- https://fonts.google.com/specimen/DM+Sans
