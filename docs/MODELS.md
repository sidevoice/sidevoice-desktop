# Models in the desktop app: native, never in the webview

In the app, speech-to-text and text-to-speech run only in its **native engine** (docs/ENGINES.md), never in the
page: a native runtime is never offered a page engine's builds (sidevoice/sidevoice-core#21 D5). The bundled interface
asks the engine what this device is (`nativeEngine.capabilities()`, docs/BRIDGE.md), resolves its offers from
that and the catalogue, and runs the chosen build through the engine. The same interface in a browser offers the
page's own engines instead (transformers.js on WebGPU or WebAssembly).

## Why not the webview (macOS)

The page engines depend on WebGPU for anything beyond the smallest models, and the system webview is not a
browser with a choice of engine:

| macOS | WebGPU in WKWebView | What a page engine could run |
|---|---|---|
| 26 (Tahoe) and later | Expected on (WebGPU ships enabled in Safari/WebKit 26) | Whisper up to large-v3-turbo on the GPU |
| 13–15 | Off (observed on macOS 14.8 in CI) | Whisper tiny/base and Kokoro q8 on the CPU via single-threaded WebAssembly |

The native engine runs the same models on every supported macOS, multi-threaded, with the same result in any
webview.

## What the settings window shows

"Motor nativo" / "Native engine": what the engine reports to the page (system, accelerators, memory) and what it
has on disk — each downloaded model build, with what it downloaded (the engine itself is linked into the app). Models are downloaded the first
time the person chooses them in the room's settings.
