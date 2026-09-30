# What the app talks to: a room, or a node

Since the client/server split (rubasace/sidevoice `docs/RENDEZVOUS.md`, sidevoice/sidevoice-core) a client
talks to one of two things. Settings → "Con qué hablas" picks it.

| Kind | Example | What the window shows | Target |
|---|---|---|---|
| **Room** | `https://room.example.invalid` (today's), a split room (rendezvous + relay) | The room's own page (`/voice/`), remote | The page's own origin |
| **Node** | `http://127.0.0.1:8768` (the core the connector starts on this Mac), a reachable node over https | The interface bundled in the app (`ui/voice/`), local | `window.__SIDEVOICE_TARGET__` = the node's URL |

## Why a room opens the room's page

- It works with **every** room: the deployed one (before the split, no `/api/rendezvous`), and the split's
  rendezvous room. The page it serves is the version that room expects.
- Sign-in keeps working: the operator's room sits behind oauth2-proxy + Google, whose cookie belongs to the
  room's origin. A bundled page on `tauri://localhost` calling the room cross-origin would carry no cookie
  (WKWebView blocks third-party cookies) and every call would bounce to Google.
- The pairing panel is the room's: "Emparejar máquina" shows a code from that room.

`__SIDEVOICE_TARGET__` is not set for a room: a page served by its target resolves the target to its own
origin (`resolveTarget` in the web client), which is the same thing.

## A node opens the bundled interface

The node serves no page (by design: sidevoice-core's handover). The app ships the web interface, vendored
from rubasace/sidevoice (`scripts/vendor-web.mjs`, provenance in `ui/voice/web-source.json`) at the paths
the room serves it from (`/voice/`, `/voice-browser/`, `/voice/mic_capture.js`), and injects:

```js
if (location.protocol + '//' + location.host === "tauri://localhost") { window.__SIDEVOICE_TARGET__ = "http://127.0.0.1:8768"; }
```

The web client then asks `GET <target>/api/rendezvous` → `{kind: 'node', …}` and sends every request and the
call socket to the node, cross-origin.

What makes cross-origin work (sidevoice-core `c0060a3`):
- the core accepts the app's own origins (`tauri://localhost`; Windows `http(s)://tauri.localhost`) in its
  origin check without configuration — only an app installed on the machine carries them, and anything
  installed there already reaches loopback;
- the core answers CORS (and preflights, and Chromium's private-network preflight) for the origins it accepts;
- the call socket is a WebSocket: the core's origin check is what applies.

A node on another machine needs https (mixed content) and the node's `SIDEVOICE_ALLOWED_HOSTS`.

### Pairing, pointed at a node

In a room, "Emparejar máquina" pairs *another* machine with the room. Pointed at a node, the page is talking
to the machine itself, so the panel pairs **this machine with a room** instead (room address + the code that
room shows): `POST <node>/api/rendezvous/pair` → the core asks its connector (`pair.request`) → the connector
does what `voice_pair` does and writes `credentials.json` → the core's rendezvous dials the new room at once.
The panel also shows which room the machine is paired with and whether the link is up.

## The app's side

- `src-tauri/core/src/settings.rs`: `TargetKind`, `page_origin`, `target_script`, `url_origin` (the
  WHATWG origin of `tauri://localhost` is opaque; pages report `scheme://host`, and so do these).
- `src-tauri/src/lib.rs` `open_room`: remote URL vs `WebviewUrl::App("voice/index.html")`; the bridge
  capability is remote (room origin) or local (app pages); the microphone rule and navigation follow the
  page origin.
- The bundled interface's in-browser models need ONNX Runtime and espeak-ng WebAssembly at
  `/voice-browser/assets/`: `scripts/web-assets.mjs` copies them from the pinned npm packages before each
  build (not committed; 50 MB).

## Updating the bundled interface

```sh
W=<rubasace/sidevoice checkout>
(cd $W && npm run build -w @sidevoice/protocol && npm run build -w @sidevoice/browser-audio && npm run build -w @sidevoice/web)
node scripts/vendor-web.mjs $W
```

If `onnxruntime-web` or `espeak-ng` change version in `packages/browser-audio`, change the exact versions in
this repo's `package.json` too.
