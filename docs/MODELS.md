# In-browser models inside the desktop webview

The web UI can run speech-to-text (Whisper, transformers.js) and text-to-speech (Kokoro) in the
page itself. Inside the desktop app "the page" is the system webview: **WKWebView on macOS**,
WebView2 on Windows, WebKitGTK on Linux. The app adds no model code of its own.

## What the web UI does already (rubasace/sidevoice `packages/browser-audio`)

- `stt-engine.js` asks `navigator.gpu.requestAdapter()`. With an adapter it offers WebGPU models;
  without one it offers only the WASM ones: **Whisper tiny and base** (q8). Whisper small and
  large-v3-turbo are WebGPU-only and are simply not listed.
- `worker.js` / `engine.js` (Kokoro): `device: 'auto'` → `webgpu` if `navigator.gpu` exists,
  otherwise `wasm` (q8).
- The join status line reports "GPU" or "CPU" once a model is ready.

So the fallback is already explicit in the web UI; the desktop app does not need its own.

## WKWebView and WebGPU (macOS)

| macOS | WebGPU in WKWebView | What runs |
|---|---|---|
| 26 (Tahoe) and later | Expected on (WebGPU ships enabled in Safari/WebKit 26) | Whisper up to large-v3-turbo on the GPU, Kokoro fp32 on the GPU |
| 13–15 | Off (it was behind a feature flag) | Whisper tiny/base and Kokoro q8 on the CPU via WebAssembly |

This could not be checked on a Mac tonight. **The settings window shows the real answer on each
machine** ("Este ordenador → WebGPU"): it runs the same `navigator.gpu.requestAdapter()` in the
same engine the room page uses.

WebAssembly runs single-threaded unless the page is cross-origin isolated (COOP/COEP headers);
the room does not send them today, exactly as in Safari. Enabling them on the server would speed
up the CPU path in every browser, not only here.

Models are cached by the webview (Cache API) in the app's persistent data store, so they download
once per machine, not per launch.

## The seam for a native engine (not built)

When the webview is too slow (no WebGPU), a native engine can run next to the app:

1. **Process**: a sidecar binary shipped in the bundle (`bundle.externalBin` in
   `tauri.conf.json`, started with `tauri-plugin-shell` or `std::process`) — ONNX Runtime with
   CoreML on macOS, the same models. It listens on loopback only.
2. **Announcement**: the app sets `window.__sidevoiceDesktop.host.nativeEngine` (today `null`,
   in `bridge/desktop-bridge.js`) to `{ url: "http://127.0.0.1:<port>", token, models: [...] }`.
3. **Use**: the web UI, when it sees a native engine, offers it as one more "where does this model
   run" choice next to "this browser" and "the server" — the per-device, per-server choice the
   architecture brief already asks for. It speaks to it like to any provider.

Nothing in the app assumes the engine exists; adding it touches the bundle config, one Rust
module, and the `host` object.
