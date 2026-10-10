# What the app shows, and what it talks to

By design:
- The desktop app **bundles the web interface** (the sidevoice-web client). It never
  loads a remote page. The interface is a secret-free static client; the same build is also a standalone static
  site (sidevoice-web `deploy/web-static/`) for people who cannot install the app.
- **Auth is device pairing** with the node: the node issues a one-time code (its agent shows it with the MCP
  tool `voice_pair_device`, or `sidevoice pair-device` on the machine), and the person pastes it in the
  interface (Máquinas → Emparejar). The code carries the node's id, how to reach it (its own URLs and/or a
  rendezvous room), a one-time secret and the fingerprint of the node's identity key, which the interface pins.
  The node side of the contract is sidevoice-core's `server/devices.py`.
- No oauth2-proxy / Google sign-in in the app.

## The window

`room-N` shows `tauri://localhost/voice/index.html` (Windows: `http://tauri.localhost/voice/index.html`), the
interface of the sidevoice-web release `web.pin.json` names: that release's static site (`sidevoice-web-<version>.tar.gz`),
unpacked into `ui/voice/` in the layout a room serves (`scripts/build-web.mjs`; `ui/voice/web-source.json` says
which release). None of it is committed, and nothing of it is built here.

The window only ever navigates within the app's own pages; the microphone is granted to them only; the bridge
(`docs/BRIDGE.md`) is bound to them.

## The target (optional)

Settings → "Dónde mirar primero": a node (`http://127.0.0.1:8768` for the core on this Mac) or a rendezvous room
(`https://…`). The app injects it as `window.__SIDEVOICE_TARGET__` (the web client contract) into the app's own
pages only. Empty is fine: the pairing code says where the machine is. A standalone static deployment sets it by
writing `/voice/target.js` (`SIDEVOICE_TARGET` in the nginx image).

`https://`, or `http://` only on this machine (a secure page may not call plain http elsewhere).

## What the node and the room accept from the app

- The node (sidevoice-core) accepts the app's origins (`tauri://localhost`, `http(s)://tauri.localhost`) in its
  origin check without configuration and answers CORS for them, preflights included.
- Everything the interface calls on a node requires the device token from pairing (`Authorization: Bearer …`;
  the call socket carries it as a WebSocket subprotocol). Only discovery, redeeming a code and the identity
  proof are open. The room relays requests and the socket to the node and passes the token through; it checks
  nothing itself — the node does, end to end — and no longer lists nodes or machines to pages.

## Verified in CI (real WKWebView, macOS 14 runner)

- First open with no settings: the bundled interface loads and its controller comes up (the bridge reports
  `ready`).
- The interface's window is a secure context and WebCrypto ECDSA P-256 (what pinning needs) works (probe page,
  same window and rule). Where the app runs the call itself (`host.voice`, macOS) the page is never granted the
  microphone: the app owns it.
- The voice call: `host.voice`'s seam on the probe page, and a call on the engine's real models through the app's
  adapter, recorded speech in and a spoken reply out (`src-tauri/src/voice/models/tests.rs`).
- With a target, the interface calls it cross-origin with `Origin: tauri://localhost` (fake node).

## Updating the bundled interface

Move `web.pin.json` to another sidevoice-web release: its `tag` (`vX.Y.Z`, or `nightly`) and the SHA-256 of its
tarball, as the release's `SHA256SUMS` lists it; then `npm run web`. The next Tauri build or dev run does the same by
itself. The tarball must match that digest and be listed so in `SHA256SUMS`, and its attestation
(`attestation.sigstore.json`) is verified with `gh attestation verify` when `gh` is on the PATH (always in CI). The
unpacked site is kept in `target/web/<sha256>/site/`, so a second run at the same pin only copies it. A `nightly` pin
is only ever temporary: its tarball is replaced on every push to sidevoice-web's main, and the digest stops matching.
