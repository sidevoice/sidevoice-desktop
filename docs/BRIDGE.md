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
| `src-tauri/src/voice.rs`, `src-tauri/src/keychain.rs`, `src-tauri/core/src/voice.rs` | The voice call the app runs, behind `voice` (macOS; below), its provider keys in the keychain, and the builds it picks (unit-tested). |
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

`snapshot` (version 2):

```json
{ "version": 2, "ready": true, "joined": true, "busy": false,
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

`window.__sidevoiceDesktop.host` is `{ app: "sidevoice-desktop", version, nativeEngine, mediaKeys, localHost?, voice? }`: the
web UI feature-detects the desktop app by it, and each capability by its presence. `version` is the bridge's (2).

## The local host

`host.localHost` is there only where the app offers this computer's own core (macOS, design O2); elsewhere it is
absent and the page hides what needs it. Nothing runs until the page calls it. Contract, states and actions:
docs/LOCAL_HOST.md.

| Call | Returns / does |
|---|---|
| `state()` | `{state, failure?, core?, service?, calls?, attempts?, limit?, reachable}` (`limit` only where the service manager has one: «(2 de 5)» only then); `state` one of `absent`, `not-installed`, `stopped-by-person`, `starting`, `backoff`, `running`, `failed`, `service-failed`, `refused`, `incompatible` |
| `subscribe(listener)` → `stop` | the state now, then on every change (polled every 2 s while anyone listens) |
| `pairing()` | `{fp, public_key, device_id, token, urls: ["http://127.0.0.1:<port>"], rv: null, host, local: true}` whenever a core answers and the app is paired (`reachable`), whatever `state` says, else `null`; `token` is the app's proxy secret for this launch, never the device token |
| `start()` / `stop()` / `restart()` / `serviceInstall()` / `serviceUninstall()` | the connector's `service …`; resolve the state after it |
| `reconnect()` | pairs the app with the core again (after `refused`; never done on its own) |
| `revealLog()` | shows the core's log in Finder (`~/.sidevoice/core.log`, else `connector.log`) |
| `pairingCode()` | `{code, expires_in, reach}`, on the person's click only: shown, never sent — the page's convention; native authorises the caller, not a person (docs/LOCAL_HOST.md → Trust) |
| `pairRoom(url, code)` | pairs this machine with a room (`pair … --json` only): `{room}` |

Actions reject `{key, message}`. Commands behind it (`src-tauri/src/local_host.rs`): `local_host_state`,
`local_host_pairing`, `local_host_action {action}` (`start`, `stop`, `restart`, `service-install`,
`service-uninstall`, `reconnect`, `reveal-log`), `local_host_pairing_code`, `local_host_pair_room {url, code}`.
Granted to the room window (`capabilities/room.json`); each re-checks that the caller is the current room window on
the app's own page.

## The call controls card

During a call, while the room's window is not in front and focused, a small card floats over the person's other apps
(sidevoice/sidevoice-desktop#4): the agent, the conversation, the person's microphone, and near the pointer the call's
controls. Hidden from the tray ("Hide call controls") it stays hidden for that call; "Show the call controls always"
in the settings keeps its controls in view.

- **The window** (`src/call_controls.rs`, label `call-controls`) loads `voice/call-controls.html`, a second page of
  the bundled web build (`apps/web/src/call-controls/`), which reuses the room's conversation list and icons.
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

The app's engine (docs/ENGINES.md), for what the page manages of its models: what this device runs, which builds are
on disk, and getting one there with its progress. Running models is the voice call's (below), never the page's.
`window.__sidevoiceDesktop.host.nativeEngine`:

| Call | Returns / does |
|---|---|
| `capabilities()` | `{runs: "native", os: "macos"\|"windows"\|"linux", arch: "aarch64"\|"x86_64", has: ["cpu", …], memory_mb: number\|null}` — `has` is what sidevoice-engine runs a model on here (`cpu`; `metal` on Apple Silicon, for whisper.cpp; `remote` for the providers' models), never Core ML, `memory_mb` the machine's total, from the OS |
| `installed()` | `[{model, engine}]` — builds whose files are on disk and whole |
| `install(model, engine, onProgress?)` | downloads the model's build, whatever of it is missing. Returns a promise that also carries the install's job id from the start, `promise.job` (a string), for `cancel`. `onProgress(event)` about twice a second, only with this call's own bytes (below) |
| `cancel(job)` | cancels that install, waiting or running; its `install` promise rejects with `{key: "install_cancelled", message}` and the model is not on disk afterwards (unless it already was before the install). Resolves `true` when the install will reject so — also when it has not reached the app yet (it is refused as it arrives) — and `false` when it had already ended (a no-op, never a rejection) |
| `memory()` | `{total_mb, available_mb}`: the machine's memory and what is available of it now, each `null` when the OS does not say. On macOS `available_mb` is the share the kernel's memory-pressure level reports free (`kern.memorystatus_level`): a gauge, not a limit, since macOS compresses and swaps rather than fail |

- `model` is the catalogue id (`whisper-small`, `kokoro-82m-v1.0`), `engine` the build's engine id (`sherpa-onnx`,
  `whisper-cpp`): sidevoice-engine's model id and backend id. The app installs exactly that build or refuses it
  (below), and never another build instead. Before it downloads anything it checks, at this trust boundary, that the
  build runs here: an engine in this app, the build's needs and accelerators, and the model's memory against the
  machine's (unknown memory is not a refusal).
- Installs run one at a time. Each `install` call is its own job (`job`, an id the bridge makes, `promise.job`); the
  app keeps progress per job from the moment the job starts — after any install ahead of it — until it ends, so a
  call that waits reports nothing and no call ever reports another's bytes. Each report is one object:

  ```json
  { "job": "install-3-mg8k2x", "model": "whisper-small", "engine": "sherpa-onnx",
    "done": 52428800, "total": 639387718, "bytes_per_s": 31457280 }
  ```

  `done`/`total` are this job's bytes across the build's files; `total` comes from sidevoice-engine's catalogue, so it
  is known from the first report (0 when nothing is left to fetch).
  `bytes_per_s` is `null` until the first second is measured, then the speed sampled each second, each sample
  weighing half against the speed before it; time left is `(total - done) / bytes_per_s`.
- A cancelled install stops within a moment, whatever it is doing: waiting for another install, waiting on the
  network — for the answer's headers or its next chunk, however long the server has gone quiet: the request is
  dropped and its connection closed — or unpacking. It frees the install lock, removes what it was downloading
  (sidevoice-engine stores a file only once it is whole and verified) and rejects with `install_cancelled`. A file it
  had already completed is whole and verified, and stays. A cancel the app accepted always wins: one that lands as
  the files are moved into place removes the model again, and a transport error that follows it (the connection it
  closed) is still `install_cancelled`, never `download_failed`. There is no overall download timeout: a slow line is
  not a failure, and the cancel is how a person stops one.
- Commands behind it (`src-tauri/src/engine_ipc.rs`): `engine_capabilities`, `engine_installed`,
  `engine_install {model, engine, job}` with `engine_progress {job}` (the report above, or `null` while the job
  waits or after it ends) and `engine_cancel {job}` (`true`/`false` as `cancel`), `engine_memory`. Granted to the room
  window (`capabilities/room.json`). The settings window may read `engine_capabilities` and `engine_on_disk` (what is
  on disk and its size), and nothing else of the engine.

### Refusals

A call the app refuses, or one that fails, rejects with the shape of sidevoice-core's refusals: a stable `key`
the page translates, the parameters its message needs beside it, and `message`, the sentence in English for a
client that does not know the key:

```json
{ "key": "model_needs_memory", "model": "whisper-large-v3", "needed_mb": 4000, "memory_mb": 2000,
  "message": "whisper-large-v3 needs 4000 MB of memory; this machine has 2000 MB." }
```

The page renders by `key` and falls back to `message`. The engine's keys are made in `src-tauri/engine/src/error.rs`,
the voice call's in `src-tauri/src/voice.rs` and core `voice.rs`; a new one is added there and here. What
sidevoice-engine itself fails with is one of these keys, with the engine's own code beside it as `code`
(`digest-mismatch`, `model-load-failed`…).

| `key` | Parameters | When |
|---|---|---|
| `engine_unsupported` | `engine` | no runtime in this app for that engine: unknown, a page's engine, or one a newer catalogue added |
| `model_unknown` | `model` | not in sidevoice-engine's catalogue |
| `build_missing` | `model`, `engine` | the model has no build for that engine |
| `build_unfit` | `model`, `engine`, `reason` (the engine's code, when it gave one) | the build needs a feature, or an accelerator, this device lacks |
| `model_needs_memory` | `model`, `needed_mb`, `memory_mb` | the model needs more memory than this machine has |
| `model_wrong_task` | `model`, `task` | the engine refused a model for what it was asked to do |
| `not_installed` | `model`, `engine` | the engine found a file missing |
| `download_failed` | `model`, `engine` | the network failed, or answered with an error |
| `download_corrupt` | `model`, `engine` | the bytes are not the ones the catalogue names (SHA-256) |
| `install_failed` | — | unpacking or moving the download into place failed (disk, permissions, an archive without its files) |
| `install_cancelled` | — | the page cancelled the install (`cancel(job)`): not a failure, nothing to show as an error |
| `runtime_failed` | `engine` | the engine refused, with a code this app does not map |
| `voice_model_unknown` | `model` | a voice setting names a model the engine's catalogue does not have |
| `voice_model_wrong_task` | `model` | a voice setting names a model that cannot do that stage (a voice for transcription) |
| `voice_model_unfit` | `model` | no build of that model runs on this device |
| `voice_build_unfit` | `model` | a voice setting names a build that is not one of the model's that runs here (Core ML and MLX included) |
| `voice_end_of_turn_unavailable` | — | `end_of_turn: "smart-turn"` while no model of the catalogue ends turns (sidevoice-engine#69) |
| `voice_settings_missing` | `code` (`settings-missing`) | a call of `host.voice` before its first `setSettings` |
| `voice_failed` | `code` (the call's: `microphone-denied`, `audio-device-unavailable`, the engine's `model-load-failed`, …, or `stopped`) | the call could not start, or was stopped before it listened |
| `keychain_failed` | — | the keychain refused to keep or read a key |
| `engine_unavailable` | — | the engine did not start with the app |
| `bad_request` | — | a malformed call, or a caller other than the room window's own page: a bug in the caller |
| `internal` | — | something inside the app failed |

## The voice call

On macOS the app runs the voice call itself: sidevoice-voice (`sidevoice/sidevoice-voice`) on the app's engine, with
the device's own microphone and speaker (cpal) and WebRTC's AEC3 between them, so the call cancels its own echo
(`src-tauri/src/voice.rs`; sidevoice/sidevoice-core#89). The room window's page is then never granted the microphone
(`media::decide`) and runs no model: it keeps the room. It hands the call the room's replies and carries the call's
turns and playback reports to the room, in its outbox. Elsewhere `host.voice` is absent, the page runs the call
itself over `@sidevoice/voice` in the webview, and the room window grants it the microphone as before.

### The seam

`window.__sidevoiceDesktop.host.voice` implements `VoiceHost`, the page's voice seam, defined once in sidevoice-voice:
[`js/voice-host.d.ts`](https://github.com/sidevoice/sidevoice-voice/blob/feat/voice-host/js/voice-host.d.ts) in
`@sidevoice/voice` (sidevoice/sidevoice-voice#6), with its payload types (`VoiceSettings`, `VoiceUserTurn`,
`VoicePlayback`, `VoiceReply`, `VoiceState`, `VoiceKaraoke`, `VoiceHostError`) and what every implementation promises
(that package's README, "The voice seam a page drives"). On the web the same package's `createVoiceHost(engine)`
implements it over the call in the page; the page does `host.voice ?? createVoiceHost(await WebEngine.create(host))`
and uses nothing else of either. It is not restated here: a change is made there, and both sides follow.

What is this app's own:

- `setSettings` makes sidevoice-voice's configuration (core `voice.rs`): a build the person names, if it runs here;
  otherwise Whisper on whisper.cpp (Metal on Apple Silicon) and other models on the engine's recommended build; never
  Core ML nor MLX; Silero for voice activity; `smart-turn` only with a model of the `end-of-turn` capability. It
  rejects with the app's keyed refusals (below), each with the seam's `code` beside its `key` (`voice_model_unknown`
  → `model-unknown`).
- `start` rejects `{key: "voice_failed", code, message}` with the call's code, `{key: "voice_settings_missing", code:
  "settings-missing"}` before any settings, and `{key: "voice_failed", code: "stopped"}` when `stop` came first.
- Provider keys go to the macOS keychain; the page never reads one back.
- A page that loads anew finds the call stopped, its models still loaded.

### How it travels

- Page → app: the commands `voice_set_settings {settings}`, `voice_start`, `voice_stop`, `voice_speak {reply}`,
  `voice_set_online {online}`, `voice_mute {muted}`, `voice_cancel_input`, `voice_models`,
  `voice_set_provider_key {provider, key}`, `voice_has_provider_key {provider}` (`src-tauri/src/voice.rs`). Granted to
  the room window (`capabilities/room.json`); each re-checks that the caller is the current room window on the app's
  own page.
- App → page: `window.__sidevoiceDesktop.voiceEvent(event)` (`webview.eval`), each sidevoice-voice `VoiceEvent` as
  JSON, `{type: "room-message" | "state" | "level" | "karaoke" | "error", data}`; the bridge hands each to its
  listeners (a room message by its own `type`).
- Keys: `setProviderKey` writes the macOS keychain (`src-tauri/src/keychain.rs`), and the engine reads it when a
  remote model is installed, loaded or called.

## Changing the bridge

- Add a command: a variant in `Command` (Rust), a `case` in `run` (JS), a test on each side.
- Add a field: `CallSnapshot` uses `#[serde(default)]`, so old and new scripts interoperate;
  bump `version` only for a breaking change.
- Tests: `npm test` (the script against a fake window, `host.voice` included), `cargo test -p sidevoice-desktop-core`
  (the voice call's builds and the media rule among the rest) and `cargo test -p sidevoice-desktop-engine` (the engine
  over a test host), `cargo test -p sidevoice-local-host` (the local host against a stand-in core; docs/LOCAL_HOST.md →
  Tests). In the real app, CI's macOS job runs the probe page (`test/fixtures/probe.html`, probe build): the engine's
  capabilities, installs, refusals and a cancelled download (CI then checks nothing of it is on disk), and the voice
  call's seam, catalogue, builds and a start that settles. The call itself, with real models on recorded speech, is
  sidevoice-voice's own CI.
