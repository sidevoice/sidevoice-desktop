# The desktop bridge

How the native app (tray, global shortcut) talks to the Sidevoice web UI loaded in its window.
It uses the seams the web UI already publishes for itself, and never reads or clicks the DOM.

## Pieces

| Where | What |
|---|---|
| `bridge/desktop-bridge.js` | Injected by the app into every page of the room window, before the page's own scripts. Bound to the app's own origin (the bundled interface). |
| `src-tauri/core/src/bridge.rs` | The contract on the Rust side: `CallSnapshot`, `Command`, and how the tray renders a snapshot. Unit-tested. |
| `src-tauri/src/lib.rs` | Wiring: the `bridge_state` command, `send(Command)`; `capabilities/room.json`. |
| `src-tauri/src/tray.rs` | The menu-bar icon and its menu. |

## What the bridge reads and calls in the web UI

Both are globals the web UI (`rubasace/sidevoice` → `apps/web`) already defines:

- `window.sidevoiceUI.store` — the room's view-model store (`apps/web/src/state/room-store.ts`,
  `installRoomBridge`). The bridge calls `getState()` and `subscribe()` and reads only
  `call.joined`, `call.busy`, `mic.enabled`, `mic.disabled` and `title`.
- `window.sidevoiceActions` — the room's actions (`apps/web/src/state/room-types.ts`,
  `SidevoiceActions`). The bridge calls only `toggleMic()` and `toggleCall()`.

If the web UI renames either, the tray greys out ("sala sin cargar") and nothing else breaks.
Keeping these two names stable is the web UI's side of the contract.

## Page → app

```js
window.__TAURI_INTERNALS__.invoke("bridge_state", { snapshot })
```

`snapshot` (version 1):

```json
{ "version": 1, "ready": true, "joined": true, "busy": false,
  "micEnabled": true, "micDisabled": false, "title": "Claude" }
```

Sent once on page start (`ready: false`), then on every change of those fields (the store
ticks often; unchanged snapshots are not re-sent). The app also resets to `ready: false` on
every navigation of the room window.

Who may call it:
- `capabilities/room.json` grants `allow-bridge-state` (and `allow-debug-log`, printed only with
  `SIDEVOICE_DEBUG=1`) to the room windows (`room-*`), for the app's own (local) pages only.
- The command re-checks that the caller is the current room window and that its page is the app's own origin.

The room window never shows a remote page, so no remote origin can call it.

## App → page

```js
window.__sidevoiceDesktop.run("toggle-mute")   // tray "Silenciar/Activar micrófono", global shortcut
window.__sidevoiceDesktop.run("hang-up")       // tray "Colgar"
```

Evaluated by the app with `webview.eval` (works with the window hidden). `run` returns whether it
did something:
- `toggle-mute` → `sidevoiceActions.toggleMic()`, unless the web UI's own mute is disabled
  (in a call with no conversation selected). Before joining it sets the preference, as the web
  UI's button does.
- `hang-up` → `sidevoiceActions.toggleCall()`, only while joined.

Joining is deliberately **not** a tray command: joining unlocks audio output, which the webview
only allows from a click in the page. "Mostrar Sidevoice" opens the window for that.

`window.__sidevoiceDesktop.host` is `{ app: "sidevoice-desktop", nativeEngine: null }`: the web UI
can feature-detect the desktop app; `nativeEngine` is the seam for a native STT/TTS engine
(see `MODELS.md`).

## Changing the bridge

- Add a command: a variant in `Command` (Rust), a `case` in `run` (JS), a test on each side.
- Add a field: `CallSnapshot` uses `#[serde(default)]`, so old and new scripts interoperate;
  bump `version` only for a breaking change.
- Tests: `npm test` (the script against a fake window) and `cargo test -p sidevoice-desktop-core`.
