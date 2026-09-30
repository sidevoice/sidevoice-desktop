# sidevoice-desktop

The Sidevoice desktop client: a [Tauri 2](https://tauri.app) shell around the Sidevoice web UI,
so a voice call with your agent lives in its own app, with a menu-bar icon, a global mute shortcut,
and the microphone set up properly. Private while it takes shape. Part of the Sidevoice
client/server split (2026-09-30).

## What it does (v1)

- The window loads **the room (or node) URL you configure** — default the operator's room.
  First run shows a small settings screen; later, tray → *Ajustes…*.
- **Microphone**: granted by the webview to the configured origin only; `NSMicrophoneUsageDescription`
  and the audio-input entitlement for macOS. Echo cancellation, speaker output and device
  selection are the web UI's own.
- **Call app integration**: menu-bar icon with the call state (idle / live / muted), mute, hang up,
  show window, settings, quit; global mute shortcut (default ⌘⇧M / Ctrl+Shift+M); closing the
  window keeps the call and the app running; no notifications.
- The tray drives the web UI through a small **explicit bridge** (`docs/BRIDGE.md`), not the DOM.
- The web UI's in-browser models (Whisper, Kokoro) run in the system webview: WebGPU where it
  has it, WebAssembly otherwise (`docs/MODELS.md`).

## Get it

Builds come from GitHub Actions (`.github/workflows/build.yml`): open the latest run of **build**
on `main` whose macOS job is green (a newer push cancels an older run) → *Artifacts* →
`Sidevoice-macOS-AppleSilicon-dmg` (also Linux and Windows). The repo is private: downloading
needs a GitHub account with read access. The macOS build is ad-hoc signed, not notarized: read
`docs/FIRST_OPEN.txt` before opening it.

## Layout

```
bridge/desktop-bridge.js   script injected into the room page (the page side of the bridge)
ui/                        the local settings / first-run window (plain HTML, no build step)
src-tauri/src/             the shell: windows, tray, shortcut, commands
src-tauri/core/            pure logic (settings, bridge contract, media rules) — no Tauri, tested anywhere
src-tauri/Info.plist       macOS usage descriptions (merged into the bundle's Info.plist)
src-tauri/Entitlements.plist
docs/                      BRIDGE.md, MACOS.md, MODELS.md, FIRST_OPEN.txt
```

## Develop

```sh
npm ci
npm test                                   # bridge script
cd src-tauri
cargo test -p sidevoice-desktop-core       # settings, bridge contract, media rules
cargo clippy --workspace --all-targets -- -D warnings
npx tauri dev                              # needs a desktop (macOS, Windows, or Linux with WebKitGTK 4.1)
```

## Later: bundling the web UI

Today the window loads the room's own copy of the web UI (`WebviewUrl::External(room_url)` in
`open_room`). To ship the UI inside the app instead: build `apps/web` into `ui/room/`, load it with
`WebviewUrl::App("room/index.html")`, and give the web UI the server's base URL (today it calls
relative paths: `/api/...`, `/voice-browser/...`, the socket). The bridge, the tray and the
permission rules do not change; the capability and the mic rule then target the app's own origin
plus the server's.
