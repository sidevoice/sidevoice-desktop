# Third-party components

Sidevoice's Apache-2.0 license applies to its own code. The components below keep their own licenses. This
repository does not commit model weights or the generated WebAssembly runtimes; the installers do contain some of
them, as listed.

## Inside the installers

- **The Sidevoice web interface** (sidevoice-web, built at `web.pin.json` into `ui/voice/`), Apache-2.0, with its own
  dependencies: React (MIT), Radix UI (MIT), zustand (MIT), lucide-react (ISC), the OpenTelemetry JavaScript SDK
  (Apache-2.0) and the components listed in sidevoice-web's `THIRD_PARTY_NOTICES.md`.
- **Tauri** and its plugins (`tauri`, `tauri-plugin-global-shortcut`, `tauri-plugin-single-instance`), MIT or
  Apache-2.0, and the Rust crates in `src-tauri/Cargo.lock` under their own licenses (mostly MIT or Apache-2.0).
- **DM Sans**, the brand typeface (`brand/fonts/`, `ui/brand/`), SIL Open Font License 1.1 (`OFL.txt` beside it).

### The native speech engine, linked into the app binary (every platform)

- **sidevoice-engine** (Apache-2.0), a Rust dependency, and through it:
  - **sherpa-onnx** (Apache-2.0), its prebuilt static libraries, which include **ONNX Runtime** (MIT),
    **piper-phonemize** (MIT), Kaldi's decoder and feature code and OpenFst (Apache-2.0), kissfft (BSD-3-Clause),
    SentencePiece (Apache-2.0), and **eSpeak NG** with its `ucd` library, **GPL-3.0-or-later**. eSpeak NG is linked
    statically into the app's binary for the text-to-speech models that need phonemes (Kokoro, Piper):
    redistributing the installers redistributes it under the GPL. Its license text and source are at
    https://github.com/espeak-ng/espeak-ng, and sherpa-onnx's build of it is at
    https://github.com/k2-fsa/sherpa-onnx (the release `src-tauri/Cargo.lock` pins, 1.13.8).
  - **whisper.cpp** and **ggml** (MIT), compiled from source through **whisper-rs** (Unlicense).

### The voice call, linked into the app binary (macOS)

- **sidevoice-voice** (Apache-2.0), a Rust dependency, and through it:
  - **WebRTC's audio processing** (AEC3, the high-pass filter, noise suppression, gain control), BSD-3-Clause, as
    PulseAudio packages it in `webrtc-audio-processing`, compiled from source by the `webrtc-audio-processing` crate
    (its bindings under the same license), with **Abseil** (Apache-2.0).
  - **cpal** (Apache-2.0) for the microphone and speaker, and **rtrb** (MIT or Apache-2.0).

## Downloaded when the call first needs a model, not shipped

- **Silero VAD** (MIT), **Whisper** models (MIT) and **Kokoro-82M** (Apache-2.0), with **eSpeak NG's data**
  (`espeak-ng-data`, GPL-3.0-or-later) for Kokoro and Piper, from the URLs and with the licenses named in
  sidevoice-engine's catalogue (`catalog/families/` in that repository, at the version `src-tauri/Cargo.lock` pins).
- Remote models (OpenAI, ElevenLabs) are services under their providers' terms, used with the person's own key.

Review the upstream licenses and model cards before redistributing compiled assets or weights; this repository's
license does not replace those terms.

- https://github.com/espeak-ng/espeak-ng
- https://github.com/k2-fsa/sherpa-onnx
- https://github.com/microsoft/onnxruntime
- https://github.com/ggml-org/whisper.cpp
- https://gitlab.freedesktop.org/pulseaudio/webrtc-audio-processing
- https://github.com/abseil/abseil-cpp
- https://github.com/RustAudio/cpal
- https://github.com/tauri-apps/tauri
- https://github.com/snakers4/silero-vad
- https://huggingface.co/hexgrad/Kokoro-82M
- https://github.com/openai/whisper
- https://fonts.google.com/specimen/DM+Sans
