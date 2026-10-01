# sidevoice-desktop

The Sidevoice desktop client: a [Tauri 2](https://tauri.app) shell around the Sidevoice web UI,
so a voice call with your agent lives in its own app, with a menu-bar icon, a global mute shortcut,
and the microphone set up properly. Private while it takes shape. Part of the Sidevoice
client/server split (2026-09-30).

## What it does

- The window shows **the Sidevoice web interface, bundled in the app** (never a remote page). It talks to a
  node (the core on an agent's machine) or a rendezvous room, and a device is admitted by **pairing**: the
  node issues a one-time code (ask your agent "empareja un dispositivo", or `sidevoice pair-device` on the
  machine) and you paste it under Máquinas → Emparejar. docs/TARGETS.md.
- **Microphone**: granted by the webview to the app's own pages only; `NSMicrophoneUsageDescription` and the
  audio-input entitlement for macOS. Echo cancellation, speaker output and device selection are the web UI's own.
- **Call app integration**: menu-bar icon with the call state (idle / live / muted), mute, hang up, show window,
  settings, quit; global mute shortcut (default ⌘⇧M / Ctrl+Shift+M); closing the window keeps the call and the
  app running; no notifications.
- The tray drives the web UI through a small **explicit bridge** (`docs/BRIDGE.md`), not the DOM.
- Speech models (Whisper, Kokoro) run in the app's native engine, downloaded when first chosen, never in the
  webview (`docs/MODELS.md`, `docs/ENGINES.md`).

## Get it

From the repository's **Releases**: a `vX.Y.Z` release, or `nightly`, the snapshot of the latest
green `main` (`RELEASING.md`). Each has the Apple-Silicon `.dmg`, the Windows installer, the
`.deb`/`.AppImage` and `SHA256SUMS`. The repo is private: downloading needs a GitHub account with
read access. The macOS build is ad-hoc signed, not notarized: read `docs/FIRST_OPEN.txt` before
opening it.

## Layout

```
bridge/desktop-bridge.js   script injected into the room page (the page side of the bridge)
ui/                        the local settings / first-run window (plain HTML, no build step)
brand/                     the Sidevoice brand files the app is drawn from (scripts/vendor-brand.mjs)
src-tauri/icons/, src-tauri/dmg/, ui/brand/   generated from brand/ by `npm run icons` (docs/BRAND.md)
src-tauri/src/             the shell: windows, tray, shortcut, commands
src-tauri/core/            pure logic (settings, bridge contract, media rules) — no Tauri, tested anywhere
ui/voice/, ui/voice-browser/  the web interface, vendored (scripts/vendor-web.mjs)
src-tauri/Info.plist       macOS usage descriptions (merged into the bundle's Info.plist)
src-tauri/Entitlements.plist
docs/                      TARGETS.md, BRIDGE.md, MACOS.md, MODELS.md, BRAND.md, FIRST_OPEN.txt
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

## Updating the brand

`node scripts/vendor-brand.mjs <brand-resources checkout>`, then `npm run icons`. docs/BRAND.md says what is
drawn from what, and what was decided where the brand's guidelines are silent.

## Updating the bundled interface

See docs/TARGETS.md → "Updating the bundled interface".
