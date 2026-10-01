# The desktop bridge

How the native app (tray, global shortcut) talks to the Sidevoice web UI loaded in its window.
It uses the seams the web UI already publishes for itself, and never reads or clicks the DOM.

## Pieces

| Where | What |
|---|---|
| `bridge/desktop-bridge.js` | Injected by the app into every page of the room window, before the page's own scripts. Bound to the app's own origin (the bundled interface). |
| `src-tauri/core/src/bridge.rs` | The contract on the Rust side: `CallSnapshot`, `Command`, and how the tray renders a snapshot. Unit-tested. |
| `src-tauri/src/lib.rs` | Wiring: the `bridge_state` command, `send(Command)`; `capabilities/room.json`. |
| `src-tauri/src/engine_ipc.rs` | The native engine's commands, behind `nativeEngine` (below). |
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

`window.__sidevoiceDesktop.host` is `{ app: "sidevoice-desktop", nativeEngine, mediaKeys }`: the web UI
feature-detects the desktop app by it.

## The native engine

In the app, speech models run only in its native engine, never in the page (rubasace/sidevoice#124 D5). The page
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

### What stays in memory (rubasace/sidevoice#124 D13)

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
  `cargo test -p sidevoice-desktop-engine` (the engine against a fake runtime adapter). In the real app, CI's macOS
  job selects models through the vendored room itself (`test/fixtures/room-flow.js`, probe build), paired with CI's
  stand-in machine (`test/fixtures/fake-node.py`, which proves its identity): the room's own settings actions, with
  real models, for consent → download → load → two checks → in effect; a cancel mid-download; a failure (a load
  refused, `SIDEVOICE_DEBUG_REFUSE_LOAD`); a slow model declined and accepted (`SIDEVOICE_DEBUG_SLOW_TRANSCRIBE`);
  and, after a reload, the stored choice read back — asserting what is stored, in memory and on disk each time. The
  probe page (`test/fixtures/probe.html`) loads, runs two languages on one build, unloads, reads `memory()`, watches D13
  with the idle time shortened (`SIDEVOICE_DEBUG_IDLE_UNLOAD_SECS`, probe build only), and cancels a download
  mid-way (CI then checks nothing of it is on disk).
