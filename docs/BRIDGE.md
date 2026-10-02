# The desktop bridge

How the native app (tray, global shortcut, headset, the call controls card) talks to the Sidevoice web UI loaded in
its window.
It uses the seams the web UI already publishes for itself, and never reads or clicks the DOM.

## Pieces

| Where | What |
|---|---|
| `bridge/desktop-bridge.js` | Injected by the app into every page of the room window, before the page's own scripts. Bound to the app's own origin (the bundled interface). |
| `src-tauri/core/src/bridge.rs` | The contract on the Rust side: `CallSnapshot`, `Command`, and how the tray renders a snapshot. Unit-tested. |
| `src-tauri/src/lib.rs` | Wiring: the `bridge_state` command, `send(Command)`; `capabilities/room.json`. |
| `src-tauri/src/engine_ipc.rs` | The native engine's commands, behind `nativeEngine` (below). |
| `src-tauri/src/local_host.rs`, `src-tauri/local-host/` | This computer's own core, behind `localHost` (below; docs/LOCAL_HOST.md). |
| `src-tauri/src/tray.rs` | The menu-bar icon and its menu. |
| `bridge/call-controls-bridge.js` | Injected into the call controls card's window (below). |
| `src-tauri/src/call_controls.rs`, `src-tauri/core/src/call_controls.rs` | The card's window, and when it shows, where it sits and how it moves (unit-tested). |

## What the bridge reads and calls in the web UI

Both are globals the web UI (sidevoice-web → `apps/web`) already defines:

- `window.sidevoiceUI.store` — the room's view-model store (`apps/web/src/state/room-store.ts`,
  `installRoomBridge`). The bridge calls `getState()` and `subscribe()` and reads only `call.joined`, `call.busy`,
  `mic.enabled`, `mic.disabled`, `callCard` (the room's own view for the card: the call's conversation title, the
  agent's state, `youTalking`, `canSkip`), `participants` (the rows' public fields) and `audioDevices`.
- `window.sidevoiceUI.micLevel` — the microphone's level, 0–100, from the room's capture, whether or not the room is
  painted (`apps/web/src/state/mic-level.ts`). The bridge subscribes to it.
- `window.sidevoiceActions` — the room's actions (`apps/web/src/state/room-types.ts`, `SidevoiceActions`). The
  bridge calls only `toggleMic()`, `toggleCall()`, `skipReply()`, `selectParticipant(threadId)` and
  `selectAudioDevice(kind, id)`.

If the web UI renames either, the tray greys out ("sala sin cargar") and nothing else breaks.
Keeping these two names stable is the web UI's side of the contract.

## Page → app

```js
window.__TAURI_INTERNALS__.invoke("bridge_state", { snapshot })
```

`snapshot` (version 3):

```json
{ "version": 3, "ready": true, "joined": true, "busy": false,
  "micEnabled": true, "micDisabled": false, "title": "Claude",
  "agent": "idle | working | speaking", "youTalking": false, "canSkip": false, "since": 1759300000000,
  "participants": [{ "threadId": "…", "title": "…", "selected": true, "reach": "listening", "working": false,
                     "machine": "daimon", "harness": "claude", "subtitle": "", "…": "…" }],
  "devices": { "inputs": [{ "id": "default", "label": "…" }], "outputs": [], "inputId": "default",
               "outputId": "default", "available": true, "outputAvailable": false, "busy": false } }
```

- `agent`: what the agent does — `speaking` while its voice plays (it wins over its work), else `working` while the
  conversation works on a turn, else `idle`. `youTalking`: a turn of the person's is open.
- `since`: when this page saw the call joined (ms since the epoch), for the card's clock; `null` outside a call.
- `participants` and `devices` are relayed to the card as the room wrote them: the web UI owns their shape
  (`ParticipantView`, `AudioDevices`).

Sent once on page start (`ready: false`), then on every change of those fields (the store
ticks often; unchanged snapshots are not re-sent). The app also resets to `ready: false` on
every navigation of the room window.

```js
window.__TAURI_INTERNALS__.invoke("bridge_level", { level })   // 0–100, during a call, at most every 80 ms
```

The microphone's level for the card's wave: only during a call, only when it changes (a drop to 0 always goes).

Who may call it:
- `capabilities/room.json` grants `allow-bridge-state` and `allow-bridge-level` (and `allow-debug-log`, printed only
  with `SIDEVOICE_DEBUG=1`) to the room windows (`room-*`), for the app's own (local) pages only.
- Both commands re-check that the caller is the current room window and that its page is the app's own origin.

The room window never shows a remote page, so no remote origin can call it.

## App → page

```js
window.__sidevoiceDesktop.run({ command: "toggle-mute" })     // tray, global shortcut, headset, card
window.__sidevoiceDesktop.run({ command: "hang-up" })         // tray, card
window.__sidevoiceDesktop.run({ command: "skip-reply" })      // card
window.__sidevoiceDesktop.run({ command: "select-participant", threadId: "…" })            // card
window.__sidevoiceDesktop.run({ command: "select-audio-device", kind: "input", id: "…" })  // card
```

Evaluated by the app with `webview.eval` (works with the window hidden); the command travels as JSON (core
`bridge::Command`), so nothing it carries can break out of the call. `run` returns whether it did something:
- `toggle-mute` → `sidevoiceActions.toggleMic()`, unless the web UI's own mute is disabled
  (in a call with no conversation selected). Before joining it sets the preference, as the web
  UI's button does.
- `hang-up` → `sidevoiceActions.toggleCall()`, only while joined.
- `skip-reply` → `sidevoiceActions.skipReply()`, only while something plays.
- `select-participant` → `sidevoiceActions.selectParticipant(threadId)`, only for a conversation the room lists.
- `select-audio-device` → `sidevoiceActions.selectAudioDevice(kind, id)`, `kind` `input` or `output`: the room's own
  choice, so the card and the room never disagree.

Joining is deliberately **not** a tray command: joining unlocks audio output, which the webview
only allows from a click in the page. "Mostrar Sidevoice" opens the window for that.

`window.__sidevoiceDesktop.host` is `{ app: "sidevoice-desktop", version, nativeEngine, mediaKeys, localHost? }`: the
web UI feature-detects the desktop app by it, and each capability by its presence. `version` is the bridge's (3).
R4's install, progress/cancel, update, version and agents methods exist only on macOS arm64. `install()` is explicit and always
invokes the bundled connector with `--no-agents`; it does not register an agent. The app verifies the connector
resource against `src-tauri/connector-pin.json`, including the embedded core manifest identity, before running it.
Until R4-a/b publishes the signed asset and stable CLI metadata/progress contract, the checked-in pin remains pending
and macOS production packaging fails closed. Test fixtures use a stand-in executable and are not package resources.
`version()` reports update eligibility; incomplete installed R1 metadata reports `unknown`, and a newer installed
connector reports `newer-installed` without invoking the bundled installer. `version().capabilities.agents` is false,
and `agents()` rejects with `agents.unavailable` until connector R2 supplies discovery.

## The local host

`host.localHost` is there only where the app offers this computer's own core (macOS arm64, design O2); elsewhere it is
absent and the page hides what needs it. Nothing runs until the page calls it. Contract, states and actions:
docs/LOCAL_HOST.md.

| Call | Returns / does |
|---|---|
| `state()` | `{state, failure?, core?, service?, calls?, attempts?, limit?, reachable}` (`limit` only where the service manager has one: «(2 de 5)» only then); `state` one of `absent`, `installing`, `not-installed`, `stopped-by-person`, `starting`, `backoff`, `running`, `failed`, `service-failed`, `refused`, `incompatible` |
| `subscribe(listener)` → `stop` | the state now, then on every change (polled every 2 s while anyone listens) |
| `pairing()` | `{fp, public_key, device_id, token, urls: ["http://127.0.0.1:<port>"], rv: null, host, local: true}` whenever a core answers and the app is paired (`reachable`), whatever `state` says, else `null`; `token` is the app's proxy secret for this launch, never the device token |
| `start()` / `stop()` / `restart()` / `serviceInstall()` / `serviceUninstall()` | the connector's `service …`; resolve the state after it |
| `reconnect()` | pairs the app with the core again (after `refused`; never done on its own) |
| `revealLog()` | shows the core's log in Finder (`~/.sidevoice/core.log`, else `connector.log`) |
| `pairingCode()` | `{code, expires_in, reach}`, on the person's click only: shown, never sent — the page's convention; native authorises the caller, not a person (docs/LOCAL_HOST.md → Trust) |
| `pairRoom(url, code)` | pairs this machine with a room (`pair … --json` only): `{room}` |
| `install(onProgress)` | explicit `--no-agents` install; ordered `{step, done, total, cancellable}` events; promise has `.job` |
| `cancel(job)` | true only after the connector acknowledges cancellation before commit |
| `update()` / `version()` | apply an eligible bundled update / read pinned and installed metadata plus capability status |
| `agents()` | rejects with `agents.unavailable` until connector R2 supplies discovery |

Actions reject `{key, message}`. Commands behind it (`src-tauri/src/local_host.rs`): `local_host_state`,
`local_host_pairing`, `local_host_action {action}` (`start`, `stop`, `restart`, `service-install`,
`service-uninstall`, `reconnect`, `reveal-log`), `local_host_pairing_code`, `local_host_pair_room {url, code}`,
`local_host_install`, `local_host_install_progress`, `local_host_cancel`, `local_host_update`, `local_host_version`,
`local_host_agents`.
Granted to the room window (`capabilities/room.json`); each re-checks that the caller is the current room window on
the app's own page.

## The call controls card

During a call, while the room's window is not in front and focused, a small card floats over the person's other apps
(sidevoice/sidevoice-desktop#4): the agent, the conversation, the person's microphone, and near the pointer the call's
controls. Hidden from the tray ("Hide call controls") it stays hidden for that call; "Show the call controls always"
in the settings keeps its controls in view.

- **The window** (`src/call_controls.rs`, label `call-controls`) loads `voice/call-controls.html`, a second page of
  the vendored web build (`apps/web/src/call-controls/`), which reuses the room's conversation list and icons.
  macOS: a non-activating `NSPanel` (tauri-nspanel) at the status level (25), on every Space and over full-screen
  apps, never the key window: clicking it leaves the person's app active. Windows and Linux X11: borderless,
  transparent, always on top, never focused, no taskbar button. Wayland (a Wayland display, and `GDK_BACKEND` not
  `x11`): an ordinary small window — the compositor decides where and above what, and moves it when dragged
  (`start_dragging`); nothing is remembered there. Transparency needs `macOSPrivateApi` (tauri.conf.json): the app is not distributed
  through the Mac App Store.
- **The pointer**: a window that never takes focus may get no hover events, so while the card shows one app thread
  polls the cursor (every 80 ms; asleep while it is hidden) and tells the card when it is over it; on Wayland, where
  the app cannot see the pointer, `pointerInside` is `null` and the card's own events decide. Hiding the card resets
  it (`pointerInside: false`, `level: 0`).
- **Clicks elsewhere**: `outsideClicks` counts clicks in other apps while the card shows; each closes a panel the card
  has open. macOS: an AppKit global monitor (mouse buttons only, no Accessibility permission). Windows: the pointer
  thread reads the mouse buttons. Linux: not detected (a panel closes 2.5 s after the pointer leaves). Escape is not:
  a card that never takes the keyboard cannot hear it without taking focus or reading every key system-wide.
- **No capture**: the card's webview denies every permission request (microphone, camera, …): the microphone is the
  room's.
- **Its place**: the top-right corner of the display the first time; dragged, it is dropped free with a magnet (an
  edge or corner within 56 px snaps), remembered per display (`call-controls.json` in the config directory, keyed by
  the display's name and position, in that display's own logical pixels from its work area's corner). The app moves
  it on the desktop's own coordinates (physical pixels on Windows and X11, points on macOS), so displays of different
  scales agree. The controls always open below; when they do not fit, the window rises while expanded and returns
  after (core `call_controls::fit`); where it rests is always worked out from the card at rest.

The card's page talks to the app only through `window.__sidevoiceDesktop.host.callControls`, which
`bridge/call-controls-bridge.js` defines (injected, bound to the app's own origin):

| Card → app | |
|---|---|
| `subscribe(listener)` → `stop` | the whole state now, then on every change (`call_controls_ready` asks the app for it) |
| `run({command, …})` | `call_controls_run`: `open-app` (the app shows the room), or one of the room commands above, which the app parses (`bridge::Command`) before it carries it to the room |
| `layout({width, height})` | `call_controls_layout`: the card's size with its shadow margin; the window takes it and is placed to fit |
| `drag(phase)` | `call_controls_drag`: `start`, `move`, `end`; the window follows the pointer (the app reads it) and is dropped, snapped and remembered |

App → card: `window.__sidevoiceDesktop.receive(patch)` (`webview.eval`), any subset of
`{call, level, pointerInside, outsideClicks, alwaysExpanded, muteShortcut}` — `call` is the room's snapshot above, `muteShortcut` the
global shortcut as the person reads it (⌃⌥M, Ctrl+Alt+M). `capabilities/call-controls.json` grants the four commands
to that window only, and each re-checks the caller's label and origin.

CI's macOS job (probe build, `SIDEVOICE_DEBUG_PAGE=card-probe.html`) walks a call through the card's states with an
editor full screen in its own Space (`test/fixtures/fullscreen-editor.swift`), acting as the person with the system's
own input events (`test/fixtures/card-ci.swift`). It checks the panel AppKit reports (non-activating, level 25, all
Spaces, full-screen auxiliary, not key, app not active); that the window server has the card on screen in front of the
full-screen editor; that the card's page was refused the microphone; that hovering, a click on the title (its
conversations open), a click in the editor (they close: `outsideClicks`), a drag (dropped elsewhere), and mute twice
reach the app and the room while the app never becomes active, and the editor keeps both the activation and the
keyboard (it gets what is typed before and after); and that the card's hang-up ends the call and the card hides after
it. Screenshots of each state are uploaded (`call-controls-screenshots`).

What it does not show: switching between ordinary Spaces (the full-screen editor's own Space stands for one), a real
room and microphone (the probe page stands in for the room; the room's projection and its level are unit-tested in the
web repo), other displays and scales, and any platform but macOS.

## The native engine

In the app, speech models run only in its native engine, never in the page (sidevoice/sidevoice-core#21 D5). The page
resolves its offers itself — `offers(catalog, capabilities, place)` over the catalogue it carries — from what the
engine reports, and runs the build it chose through it. `window.__sidevoiceDesktop.host.nativeEngine`:

| Call | Returns / does |
|---|---|
| `capabilities()` | `{runs: "native", os: "macos"\|"windows"\|"linux", arch: "aarch64"\|"x86_64", has: ["cpu", "coreml", …], memory_mb: number\|null}` — `memory_mb` is the machine's total, from the OS |
| `installed()` | `[{model, engine}]` — builds whose engine package and model files are on disk and whole: the download's marker names its hash, and its root and every file the engine needs from it are there |
| `install(model, engine, onProgress?)` | downloads the engine package and the model's build, whichever is missing or incomplete (a download that lost a file is fetched again). Returns a promise that also carries the install's job id from the start, `promise.job` (a string), for `cancel`. `onProgress(event)` about twice a second, only with this call's own bytes (below) |
| `cancel(job)` | cancels that install, waiting or running; its `install` promise rejects with `{key: "install_cancelled", message}` and the model is not on disk afterwards (unless it already was before the install), so a `load` of it is refused `not_installed`. Resolves `true` when the install will reject so — also when it has not reached the app yet (it is refused as it arrives) — and `false` when it had already ended (a no-op, never a rejection) |
| `transcribe(model, engine, samples, sampleRate, language, accelerator?)` | text; `samples` a mono `Float32Array`, `language` empty to detect |
| `synthesize(model, engine, voice, speed, text, accelerator?)` | `{samples: Float32Array, sampleRate}` |
| `load(model, engine, accelerator?)` | loads the build into memory and resolves `{load_ms}`: how long loading it took. One already in memory is not loaded again, and answers the time its load took. Rejects with `load_cancelled` when the page unloads that build (on that accelerator, or all) before the load ends: it is then not kept |
| `unload(model, engine, accelerator?)` | frees that build's memory on `accelerator` — only that copy — or, with none named, on every accelerator it is loaded on; resolves `null`, also when it was not loaded. A transcription or synthesis already running on it finishes first; a `load` of it still under way is not kept (it rejects with `load_cancelled`), nor is the app's own preload (below) |
| `loaded()` | `[{model, engine, accelerator, since, last_used}]`: what is in memory, oldest first; `since` and `last_used` are milliseconds since the Unix epoch |
| `memory()` | `{total_mb, available_mb}`: the machine's memory and what is available of it now, each `null` when the OS does not say. On macOS `available_mb` is the share the kernel's memory-pressure level reports free (`kern.memorystatus_level`): a gauge, not a limit, since macOS compresses and swaps rather than fail |

- `model` is the catalogue id (`whisper-small`, `kokoro-82m-v1.0`), `engine` the build's engine id
  (`sherpa-onnx`). The app runs exactly that build or refuses it (below), and never another build instead. Before
  it downloads or runs anything it checks, at this trust boundary, that the build runs here: an adapter for the
  engine, a package for this OS/architecture, the build's needs and accelerators, and the model's
  `requires.memory_mb` against the machine's memory (unknown memory is not a refusal).
- `accelerator` (optional, `cpu`, `coreml`…): the one the page chose for that build (the resolver's, or the
  person's in *Avanzado*). It must be one the build can use here, or the call is refused. Without it the app uses
  the first the build can use on this device, which is what the resolver picks.
- A build is in memory once per engine, model and accelerator. The language is each call's (`transcribe`'s, the
  voice's), so `load` takes none and one loaded Whisper or Kokoro serves every language. `transcribe` and
  `synthesize` on a build that is not in memory load it first, as before: `load` is how the page has it ready before
  the first call, and measures it. A `load` that fails rejects with a refusal (below) and leaves what is in memory as
  it was.
- Installs run one at a time. Each `install` call is its own job (`job`, an id the bridge makes, `promise.job`); the
  app keeps progress per job from the moment the job starts — after any install ahead of it — until it ends, so a
  call that waits reports nothing and no call ever reports another's bytes. Each report is one object:

  ```json
  { "job": "install-3-mg8k2x", "model": "whisper-small", "engine": "sherpa-onnx",
    "done": 52428800, "total": 639387718, "bytes_per_s": 31457280 }
  ```

  `done`/`total` are this job's bytes across what it downloads (the engine package if missing, then the model);
  `total` comes from the catalogue, so it is known from the first report (0 when nothing is left to fetch).
  `bytes_per_s` is `null` until the first second is measured, then the speed sampled each second, each sample
  weighing half against the speed before it; time left is `(total - done) / bytes_per_s`.
- A cancelled install stops within a moment, whatever it is doing: waiting for another install, waiting on the
  network — for the answer's headers or its next chunk, however long the server has gone quiet: the request is
  dropped and its connection closed — or unpacking (at the next archive entry). It frees the install lock, removes
  what it was downloading (the partial file, the unpacking directory, the unmarked slot) and rejects with
  `install_cancelled`. A download it had already completed — the engine package before the model — is whole and
  verified, and stays. A cancel the app accepted always wins: one that lands as the files are moved into place
  removes the model again, and a transport error that follows it (the connection it closed) is still
  `install_cancelled`, never `download_failed`. Any other failed download is cleaned up the same way. There is no
  overall download timeout: a slow line is not a failure, and the cancel is how a person stops one.
- Commands behind it (`src-tauri/src/engine_ipc.rs`): `engine_capabilities`, `engine_installed`,
  `engine_install {model, engine, job}` with `engine_progress {job}` (the report above, or `null` while the job
  waits or after it ends) and `engine_cancel {job}` (`true`/`false` as `cancel`), `engine_transcribe` (raw f32 body; `x-model`, `x-engine`, `x-accelerator`, `x-language`,
  `x-sample-rate` headers), `engine_synthesize` (answer: raw bytes, a u32 sample rate then f32 samples),
  `engine_load {model, engine, accelerator}` (`accelerator` `null` for the resolver's),
  `engine_unload {model, engine, accelerator}` (`null` for all),
  `engine_loaded`, `engine_memory`. Granted to the room window (`capabilities/room.json`). The settings window may read `engine_capabilities` and
  `engine_on_disk` (what is on disk and its size), and nothing else of the engine.

### What stays in memory (sidevoice/sidevoice-core#21 D13)

The app enforces it itself, from the call state the room already reports (`bridge_state`, `joined`):

- While a call is on (joining counts), nothing is unloaded but what the page unloads.
- With no call on, a build unused for 10 minutes — counted from its last load or run, or from when the last call
  ended if that is later — is unloaded (`IDLE_UNLOAD`, `src-tauri/engine/src/lib.rs`).
- As the next call connects, what the app unloaded that way is loaded again, on the accelerator it had, unless the
  page has unloaded it since — even while that preload is already loading it — or has another build of that task
  (transcription, voice) in memory by then. A build the
  page loaded to check and unloaded when the check failed does not count: the one it kept comes back.
- What the app has never had in memory since it started (a fresh launch) it cannot preload: the page's choice is the
  page's. The page calls `load` as a call connects, or the first `transcribe` / `synthesize` loads it.

With `SIDEVOICE_DEBUG=1` the app prints `engine unloaded idle …` and `engine preload … load_ms=…`.

### Refusals

A call the engine refuses, or one that fails, rejects with the shape of sidevoice-core's refusals: a stable `key`
the page translates, the parameters its message needs beside it, and `message`, the sentence in English for a
client that does not know the key:

```json
{ "key": "not_installed", "model": "whisper-small", "engine": "sherpa-onnx",
  "message": "whisper-small on sherpa-onnx is not downloaded yet." }
```

The page renders by `key` and falls back to `message`. Every key is made in `src-tauri/engine/src/error.rs`; a new
one is added there and here.

| `key` | Parameters | When |
|---|---|---|
| `engine_unsupported` | `engine` | no runtime in this app for that engine: unknown, a page's engine, or one a newer catalogue added |
| `engine_no_package` | `engine`, `platform` | the engine has no package for this OS/architecture (`macos-aarch64`) |
| `family_unsupported` | `engine`, `family` | the engine's adapter here has no pipeline for the model's family |
| `model_unknown` | `model` | not in the app's catalogue |
| `build_missing` | `model`, `engine` | the model has no build for that engine |
| `build_unfit` | `model`, `engine` | the build needs a feature, or an accelerator, this device lacks |
| `model_needs_memory` | `model`, `needed_mb`, `memory_mb` | the model needs more memory than this machine has |
| `model_wrong_task` | `model`, `task` | transcribing with a voice model, or speaking with a transcription one |
| `accelerator_unusable` | `model`, `engine`, `accelerator`, `usable` (list) | the accelerator asked for is not one this build can use here |
| `not_installed` | `model`, `engine` | run before `install` finished, or after a download lost a file |
| `voice_unknown` | `model`, `voice` | the model has no such voice |
| `language_unsupported` | `language` | the model does not know that language code |
| `download_refused` | `url` | a download that is not https |
| `download_failed` | `url` | the network failed, or answered with an error |
| `download_corrupt` | `url` | the bytes are not the ones the catalogue names (SHA-256) |
| `install_failed` | — | unpacking or moving the download into place failed (disk, permissions, an archive without its files) |
| `install_cancelled` | — | the page cancelled the install (`cancel(job)`): not a failure, nothing to show as an error |
| `load_cancelled` | `model`, `engine`, `accelerator` | the page unloaded the build while it was loading — also when the load then failed: not a failure |
| `runtime_failed` | `engine` | the engine refused to load (a library failing its hash), to load the model into memory, or to run it |
| `bad_request` | — | a malformed call (a missing header): a bug in the caller |
| `internal` | — | something inside the app failed |

## Changing the bridge

- Add a command: a variant in `Command` (Rust), a `case` in `run` (JS), a test on each side.
- Add a field: `CallSnapshot` uses `#[serde(default)]`, so old and new scripts interoperate;
  bump `version` only for a breaking change.
- Tests: `npm test` (the script against a fake window; the vendored room's native worker against this bridge,
  `test/room-bundle.test.mjs`), `cargo test -p sidevoice-desktop-core` and
  `cargo test -p sidevoice-desktop-engine` (the engine against a fake runtime adapter), `cargo test -p
  sidevoice-local-host` (the local host against a stand-in core; docs/LOCAL_HOST.md → Tests). In the real app, CI's macOS
  job selects models through the vendored room itself (`test/fixtures/room-flow.js`, probe build), paired with CI's
  stand-in machine (`test/fixtures/fake-node.py`, which proves its identity): the room's own settings actions, with
  real models, for consent → download → load → two checks → in effect; a cancel mid-download; a failure (a load
  refused, `SIDEVOICE_DEBUG_REFUSE_LOAD`); a slow model declined and accepted (`SIDEVOICE_DEBUG_SLOW_TRANSCRIBE`);
  and, after a reload, the stored choice read back — asserting what is stored, in memory and on disk each time. The
  probe page (`test/fixtures/probe.html`) loads, runs two languages on one build, unloads, reads `memory()`, watches D13
  with the idle time shortened (`SIDEVOICE_DEBUG_IDLE_UNLOAD_SECS`, probe build only), and cancels a download
  mid-way (CI then checks nothing of it is on disk).
