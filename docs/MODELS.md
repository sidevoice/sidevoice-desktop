# Models in the desktop app: native, never in the webview

In the app, speech models run only natively, never in the page: on macOS the voice call itself is native
(`host.voice`, docs/BRIDGE.md → "The voice call"), and its models run on the app's engine (docs/ENGINES.md). The
page chooses models and voices from the engine's catalogue (`host.voice.models()`) and hands the person's choices to
the call (`host.voice.setSettings`); the app picks the builds. The same interface in a browser runs the call in the
page, over `@sidevoice/voice` and the page's own engine.

## Why not the webview (macOS)

The page engines depend on WebGPU for anything beyond the smallest models, and the system webview is not a
browser with a choice of engine:

| macOS | WebGPU in WKWebView | What a page engine could run |
|---|---|---|
| 26 (Tahoe) and later | Expected on (WebGPU ships enabled in Safari/WebKit 26) | Whisper up to large-v3-turbo on the GPU |
| 13–15 | Off (observed on macOS 14.8 in CI) | Whisper tiny/base and Kokoro q8 on the CPU via single-threaded WebAssembly |

The native engine runs the same models on every supported macOS, multi-threaded, with the same result in any
webview, and the native call cancels its own echo with WebRTC's AEC3 rather than relying on the webview's.

## What the settings window shows

"Motor nativo" / "Native engine": what the engine reports (system, accelerators, memory) and what it has on disk —
each downloaded model build, with what it downloaded (the engine itself is linked into the app).
