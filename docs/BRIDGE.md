absent when the connector does not say); `state` one of `absent`, `installing` (while `install` runs, whatever the service passes through), `not-installed`, `stopped-by-person`, `starting`, `backoff`, `running`, `failed`, `service-failed`, `refused`, `incompatible` |# The desktop bridge

How the native app (tray, global shortcut, headset, the call controls card) talks to the Sidevoice web UI loaded in
its window.
It uses the seams the web UI already publishes for itself, and never reads or clicks the DOM.

## Pieces

| Where | What |
|---|---|
| `bridge/desktop-bridge.js` | Injected by the app into every page of the room window, before the page's own scripts. Bound to the app's own origin (the bundled interface). |
| `src-tauri/core/src/bridge.rs` | The contract on the Rust side: `CallSnapshot`, `Command`, and how the tray renders a snapshot. Unit-tested. |
| `src-tauri/src/lib.rs` | Wiring: the `bridge_state` command, `send(Command)`; `capabilities/room.json`. The mute shortcut, behind `muteShortcut`, and the one writer of the app's settings (below). |
| `src-tauri/src/engine_ipc.rs` | The engine's commands, behind `engine` (below): its catalogues, provider keys and models on disk. |
| `src-tauri/src/voice.rs`, `src-tauri/src/keychain.rs`, `src-tauri/core/src/voice.rs` | The voice call the app runs, behind `voice` (macOS; below), the keychain the engine reads provider keys from, and the models it picks from the catalogues (unit-tested). |
| `src-tauri/src/local_host.rs`, `src-tauri/local-host/` | This computer's own core, behind `localHost` (below; docs/LOCAL_HOST.md). |
| `src-tauri/src/tray.rs` | The menu-bar icon and its menu. |
| `bridge/call-controls-bridge.js` | Injected into the call controls card's window (below). |
| `src-tauri/src/call_controls.rs`, `src-tauri/core/src/call_controls.rs` | The card's window, and when it shows, where it sits and how it moves (unit-tested). |
| `src-tauri/src/updater.rs`, `src-tauri/core/src/updater.rs`, `src-tauri/core/src/build_info.rs` | The app's updates, behind `updater`, and what this build is, `build` (below). |
| `src-tauri/src/computer_name.rs`, `src-tauri/core/src/computer_name.rs` | This computer's name, `computerName` (below; the Linux file's parsing unit-tested). |

## What the bridge reads and calls in the web UI

Both are globals the web UI (sidevoice-web → `apps/web`) already defines:

- `window.sidevoiceUI.store` — the room's view-model store (`apps/web/src/state/room-store.ts`,
  `installRoomBridge`). The bridge calls `getState()` and `subscribe()` and reads only `call.joined`, `call.busy`,
  `mic.enabled`, `mic.disabled`, `callCard` (the room's own view for the card: the call's conversation title, the
  agent's state, `youTalking`, `canSkip`), `participants` (the rows' public fields) and `facts.audioDevices` (what the
  voice lists and what is chosen: `{inputs, outputs, input, output, outputSelectable}`, `state/room-session-state.d.ts`).
- `window.sidevoiceUI.micLevel` — the microphone's level, 0–100, from the room's capture, whether or not the room is
  painted (`apps/web/src/state/mic-level.ts`). The bridge subscribes to it.
- `window.sidevoiceActions` — the room's actions (`apps/web/src/state/room-types.ts`, `SidevoiceActions`). The
  bridge calls only `toggleMic()`, `toggleCall()`, `skipReply()`, `selectParticipant(threadId)` and
  `setAudioDevices(choice)`.

If the web UI renames either, the tray greys out ("sala sin cargar") and nothing else breaks.
Keeping these two names stable is the web UI's side of the contract.

## Page → app

### Private data and reset (`host.data`, macOS)

The bundled room page receives `host.data.pairings` on macOS, Windows and Linux. Its `read()` resolves
`{pairings: [...], pendingRevokes: [...]}` and `write({pairings, pendingRevokes})` resolves `undefined`
after a bounded, private, atomic write under the app's Application Support directory. Both arrays contain
remote pairing credentials; pending revocations remain native until the remote machine acknowledges them.
Local-host pairing is derived from the verified socket and is never stored there. Neither call accepts a
path. Browser pages keep their own browser storage. `deleteAll()` and `resetReport()` are macOS-only.

`deleteAll()` is the confirmed Settings › Advanced action. Native also displays an OS-owned confirmation
with Cancel as its default button on every invocation; cancelling rejects `{key: "reset.cancelled"}` before
any uninstall or deletion. A room script calling the bridge directly cannot bypass that confirmation. It refuses a running call, service install or
model download with `{key: "reset.busy"}`, and checks the current room window and bundled origin again in
native code. It first requires a top-level integer `safe_uninstall` of at least `1` from the installed
connector's `--version --json` response. An older, silent, malformed or timed-out connector refuses with
`{key: "reset.unsupported-connector"}` before a new reset journal or uninstall is started. It runs the
**installed connector's top-level** `uninstall --json` from its verified
`install.json` command; `localHost.serviceUninstall()` is a different, narrower service-manager action.
The connector removes the service, its releases, its data and registrations it owns. Native then forgets
Sidevoice's provider keys and starts a helper from the app executable. The helper waits for the app to
exit and for its observed WebKit children to exit before removing its exact Application Support,
shared-model store's engine-owned `blobs`, `models` and `partial` children, WebKit, cache,
saved-state and log paths and its `defaults` domain. Other programs' siblings in the shared model
store remain. A WebKit child that
does not exit leaves a partial result and its storage in place. Paths are derived natively from an
exact allowlist, checked again before each top-level removal, and traversed without following
symlinks or crossing a mount/device boundary. A failed check is reported, with that directory left
in place. Connector data and release roots must be the exact default
Sidevoice roots; unsafe environment overrides or install records refuse before the installed uninstaller runs.
If installation metadata is absent, a fresh service/process check and a check for residual connector data
must both pass before the service is called absent. A retained recovery executable named by `install.json`
is used on a partial-uninstall retry. Custom profiles and separately installed cores are excluded.
No page-supplied path is deleted.

Accepted beta limit: a hostile process running in the same user account can race these checks by
moving, replacing or mounting folders during deletion, or by replacing pairing-store ancestors
during a write. This operation does not provide atomic confinement against that process; it already
has direct access to the account's files. This limit does not relax the exact-root, no-follow and
mount/device checks for a stable filesystem layout.

Before uninstall, native creates a private operation journal and takes an interprocess reset lock.
Model operations, voice model loads, credential writes, local-host actions and updater writes share an
atomic admission gate with reset. The helper inherits the lock, acknowledges readiness before the app
quits, records cleanup stages, and releases the lock only after its work ends. The helper marks the
inherited lock descriptor close-on-exec, so the relaunched app acquires the lock independently.
The helper records an awaiting-relaunch journal; only the new app after its setup publishes a complete
report. A failed startup leaves the journal as a partial report for the next launch.

`deleteAll()` resolves `{state: "needs-restart"|"partial", service: "removed"|"absent", agents,
failed: {step, key, agent?}[], pending}` after the helper starts; this is **not** final deletion success. `partial`
already identifies a Keychain or agent-registration failure. If service uninstall fails, it
rejects `{key: "reset.service-failed"}` and leaves app data for retry. `resetReport()` resolves `null` or
the helper's one-time `{state: "complete"|"partial", completed: string[], failed:
{step, key, agent?}[], agents}` after the fresh app starts. Its report is held in a private temporary
directory until read, including the pre-exit failures. The UI must name any failed step and its remaining data.
The current installed connector reports no per-agent rows in its successful JSON response, so an empty
`agents` object does not prove that every registration was removed. The confirmation always states that
manual or foreign registrations may remain.
The app bundle, remote machines, foreign/manual agent registrations, provider accounts, OS permissions and
backups are outside this action; the connector can leave nonpersonal stop/lock markers and open conversation
servers until they exit. The UI must state these limits before confirmation.

Commands: `data_pairings_read`, `data_pairings_write {value}`, `data_delete_all`,
`data_reset_report`; only the room capability grants them and each checks the room origin.

### Call state

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
  "devices": { "inputs": [{ "id": "", "label": "", "system": true }, { "id": "CoreAudio:…", "label": "…" }],
               "outputs": [{ "id": "", "label": "", "system": true }], "inputId": "CoreAudio:…",
               "outputId": "", "available": true, "outputAvailable": false, "busy": false } }
```

- `agent`: what the agent does — `speaking` while its voice plays (it wins over its work), else `working` while the
  conversation works on a turn, else `idle`. `youTalking`: a turn of the person's is open.
- `since`: when this page saw the call joined (ms since the epoch), for the card's clock; `null` outside a call.
- `participants` are relayed to the card as the room wrote them: the web UI owns their shape (`ParticipantView`).
- `devices` is the card's `AudioDevices` (web `call-controls/host.ts`), made from the room's `facts.audioDevices`
  (`null` until the voice has listed them): the system's default first, as an option `{id: "", system: true}`; a
  device listed without a name as `{label: "", number}` (its place, from 1); `inputId`/`outputId` the chosen ids, `""`
  for the default; `outputAvailable` the voice's `outputSelectable` (when false, `outputId` is `""`); `available`
  always `true`; `busy` while a device the card chose has not been answered by the room.

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
- `select-audio-device` → `sidevoiceActions.setAudioDevices({[kind]: id})`, `kind` `input` or `output`, `id` `""` the
  system's default (`null` to the room): the room's own choice, kept for this device as its device bar keeps it, so
  the card and the room never disagree. A choice the room refuses leaves the device it had, which the card says.
  Choices go to the room one at a time, in the order asked (the room keeps whichever answers last), and the card's
  `devices.busy` holds until the last has been answered.

Joining is deliberately **not** a tray command: joining unlocks audio output, which the webview
only allows from a click in the page. "Mostrar Sidevoice" opens the window for that.

`window.__sidevoiceDesktop.host` is `{ app: "sidevoice-desktop", version, engine, mediaKeys, build, muteShortcut, callControls, computerName?, localHost?, voice?, microphone?, updater? }`: the
web UI feature-detects the desktop app by it, and each capability by its presence. `version` is the bridge's (2).

`computerName` is the name the person gave this computer, which the page proposes as the device's name: on macOS
the Computer Name (System Settings → General → Sharing, «Mac de Ana»; SystemConfiguration's
`SCDynamicStoreCopyComputerName`), not the `.local` hostname; on Windows the device name (the DNS host name, as typed);
on Linux the pretty hostname (`PRETTY_HOSTNAME` in `/etc/machine-info`, `hostnamectl set-hostname --pretty`), else
the hostname. A non-empty string, trimmed; absent when the system has none, and the page then names the device
itself. Read each time the room window opens (`src-tauri/src/computer_name.rs`, core `computer_name`).

## The local host

`host.localHost` is there only where the app offers this computer's own core (macOS, design O2); elsewhere it is
absent and the page hides what needs it. Nothing runs until the page calls it. Contract, states and actions:
docs/LOCAL_HOST.md.

| Call | Returns / does |
|---|---|
| `state()` | `{state, failure?, core?, product_version, service?, calls?, attempts?, limit?, installed?, reachable}` (`limit` only where the service manager has one: «(2 de 5)» only then; `product_version` the installed Sidevoice's version, the `sidevoice` package — not `core.version` —, `null` when unknown or not installed; `installed` whether the service starts at the next sign-in — the connector's `start_at_login`, absent when the connector does not say); `state` one of `absent`, `not-installed`, `stopped-by-person`, `starting`, `backoff`, `running`, `failed`, `service-failed`, `refused`, `incompatible` |
| `subscribe(listener)` → `stop` | the state now, then on every change (polled every 2 s while anyone listens) |
| `pairing()` | `{fp, public_key, device_id, token, urls: ["http://127.0.0.1:<port>"], rv: null, host, local: true}` whenever a core answers and the app is paired (`reachable`), whatever `state` says, else `null`; `token` is the app's proxy secret for this launch, never the device token |
| `start()` / `stop()` / `restart()` / `serviceInstall()` / `serviceUninstall()` | the connector's `service …`; resolve the state after it |
| `setStartAtLogin(enabled)` | whether the service starts at the next sign-in, and only that (a running service keeps running, a stopped one stays stopped): the connector's `service login on\|off`, which it does not ship yet — until then it rejects `service.login-unsupported`; the state it resolves reads the change back in `installed`; a non-boolean rejects `bad_request` (docs/LOCAL_HOST.md → Start at sign-in) |
| `reconnect()` | pairs the app with the core again (after `refused`; never done on its own) |
| `revealLog()` | shows the core's log in Finder (`~/.sidevoice/core.log`, else `connector.log`) |
| `pairingCode()` | `{code, expires_in, reach}`, on the person's click only: shown, never sent — the page's convention; native authorises the caller, not a person (docs/LOCAL_HOST.md → Trust) |
| `pairRoom(url, code)` | pairs this machine with a room (`pair … --json` only): `{room}` |
| `install(onProgress?)` | «Prepare this computer»: installs Sidevoice from the npm registry, without Node (docs/LOCAL_HOST.md → Install). Answers at once a promise of the state after it — once the app is paired with the new core, `reachable` — that carries `job`, this call's id (sidevoice-web's `LocalHostInstallOperation`). `onProgress({step, done, total, cancellable})` about twice a second while it runs, when that changes (below). One at a time: another call meanwhile rejects `install.busy` |
| `cancel(job)` | stops install `job` while that is honoured — `download`, `verification` and the unpacking in `staging`, never once the package's installer runs — and resolves whether it was (`true`: the install rejects `install.cancelled`); `false` too for a job not under way |

Actions reject `{key, message}`. Commands behind it (`src-tauri/src/local_host.rs`): `local_host_state`,
`local_host_pairing`, `local_host_action {action}` (`start`, `stop`, `restart`, `service-install`,
`service-uninstall`, `start-at-login-on`, `start-at-login-off`, `reconnect`, `reveal-log`), `local_host_pairing_code`,
`local_host_pair_room {url, code}`, `local_host_install {job}`, `local_host_install_progress {job}` (the progress, or
`null` for another job or once it has ended), `local_host_install_cancel {job}`.
Granted to the room window (`capabilities/room.json`); each re-checks that the caller is the current room window on
the app's own page.

**The install's progress**: `step` is `download` (`done` and `total` its bytes, `total` `null` when the registry does
not say), `verification`, `staging` (unpacking, then the package's installer), `service-start` (from when the installer
has written `install.json` until it answers) or `pairing` (the app pairing with the new core); `done`/`total` are `null`
outside `download`; `cancellable` whether `cancel` would still be honoured.

**Its failures** reject `{key, step?, params?, log_tail?, message}`: `step` where it failed, `params.check` the check
that failed for `install.authenticity`, `log_tail` the installer's last lines of error output (when it ran), `message`
an English sentence for a client that does not know the key. The keys:

| Key | When |
|---|---|
| `install.cancelled` | `cancel(job)` was honoured |
| `install.busy` | another install is under way — this app's, or one the installer's own lock holds (its `service.busy`) |
| `install.network` | the registry could not be reached, answered an error, or the download broke off, came short or took over 15 min |
| `install.no-bundle` | no release for this computer: the latest `sidevoice` lists no package for it, the registry has none, or the package carries no installer (also the installer's `core.package-missing`) |
| `install.authenticity` | `params.check`: `manifest` (a registry document that is not one, no SHA-512 integrity; the installer's `core.package-mismatch`), `download` (a tarball URL or a redirect off the registry), `download-size` (over 512 MiB), `archive-path`, `archive-link`, `archive-type`, `archive-size` (docs/LOCAL_HOST.md → Install) |
| `install.checksum` | the tarball's SHA-512 is not the registry's integrity; nothing of it was unpacked |
| `install.disk` | the disk is full |
| `install.unsafe` | the temporary directory is not private to this user (or its ancestry is not) |
| `launch.failed` | the package's installer could not be started |
| `cli.timeout` | the installer did not finish within 3 min (it was killed) |
| `install.rollback` | the installer's `install.rolled-back`: the update did not start and the previous release runs again |
| `install.rollback-failed` | the update did not start, and neither did the previous release |
| `service.start.failed` | the installer's `install.verify-failed`: installed, but the service did not start |
| `install.failed` | anything else: the app's own file errors, the installer's `install.failed` or `connector.failed`, an installer that answered nothing |
| `unsupported`, `not_room_page`, `bad_request` | as for every command |

Any other key the installer answers comes through as it is, with its `params` and `message`.

## The call controls card

During a call, while the room's window is not in front and focused, a small card floats over the person's other apps
(sidevoice/sidevoice-desktop#4): the agent, the conversation, the person's microphone, and near the pointer the call's
controls. Hidden from the tray ("Hide call controls") it stays hidden for that call. "Always show the call controls" (the
page's `host.callControls`, below) shows it during a call even while the room's window
is in front (core `call_controls::wanted`).

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
| `run({command, …})` | `call_controls_run`: `open-app` (the app shows the room), or one of the room commands above, which the app parses (`bridge::Command`) before it carries it to the room. After `select-audio-device` the card hears its devices `busy` at once, until the app next tells it the call (or 3 s pass): the device still chosen is never read as the room refusing the new one |
| `layout({width, height})` | `call_controls_layout`: the card's size with its shadow margin; the window takes it and is placed to fit |
| `drag(phase)` | `call_controls_drag`: `start`, `move`, `end`; the window follows the pointer (the app reads it) and is dropped, snapped and remembered |

App → card: `window.__sidevoiceDesktop.receive(patch)` (`webview.eval`), any subset of
`{call, level, pointerInside, outsideClicks, muteShortcut}` — `call` is the room's snapshot above, `muteShortcut` the
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

The app's engine (docs/ENGINES.md), for what the page knows and manages of its models: its catalogues and the keys of
remote providers, what this device runs, which builds are on disk, and getting one there with its progress. Running
models is the voice call's (below), never the page's. `window.__sidevoiceDesktop.host.engine` is the page's
`ModelCatalogs` seam in the app (sidevoice-web `apps/web/src/services/model-catalogs.js`), as that file types it —
`install` included, with its options, progress and cancel — plus what this device is, runs and has on disk:

| Call | Returns / does |
|---|---|
| `catalogs()` | `[{id, name, status: {reason?: {code, params}, stale, detail?}, models}]`: every catalogue of sidevoice-engine 0.5.0, `local` first (`name` `null`), then each provider (`openai`, `elevenlabs`; `name` the provider's), each with every model it lists in `/engine`'s shape: `{id, capabilities, languages, voices: [{id, name?, languages, gender?}], speed?: {min, max}}`, and for a local model also `family, parametersM, license, installed, lastUsedAt?, builds: [{id, backend, accelerator?, acceleratorClass?, precision, downloadBytes, memoryMb, available, reasons: [{code, params}], installed}], recommendedBuild?`. `acceleratorClass` is the engine's optional class id, exactly `"gpu" | "neural-engine" | "cpu"`; it is absent when the build has no accelerator here. `lastUsedAt` is when the engine last loaded the model, in milliseconds since the Unix epoch, a number; absent while it does not know: not installed, or not loaded since it was. A catalogue that cannot list (a provider with no key, one that refuses) has `models: []` and says why in `status.reason` (`credential-missing`, ...), the provider's own words in `status.detail`. Rejects `{code}` only when the engine itself fails |
| `setCredential(provider, key)` | keeps `key` for remote catalogue `provider` in the macOS keychain, or removes it (`null` or blank), then lists that provider again with it. Rejects `provider_invalid`, `credentials_failed`, and `credentials_unavailable` where the app keeps no keys (Windows, Linux) |
| `hasCredential(provider)` | whether the keychain holds a key for `provider` (`false` where the app keeps none). The key itself never reaches the page |
| `testCredential(provider, key)` | asks `provider` with `key` (trimmed; blank is no key) without keeping it, and answers what its catalogue says against it, `{code, params, detail?}` (`credential-rejected`, `listing-not-permitted`, `provider-unreachable`, …, as `status.reason`; `detail`, when the provider gave some, its own words as `status.detail` has them, with every occurrence of the tried key replaced by `[key]`, control characters as spaces, and cut to 300 characters with "…"), or `null` when the provider lists with it. The key is never written to the keychain, never logged, and never reaches the listings the app keeps: the trial runs on an engine of its own, made for it and dropped after it, whose only key is this one for this provider. So the kept key, its listing, and anything listed or called at the same time (a call in progress, a `catalogs()`) are the kept key's, before, during and after. Rejects `catalog_not_found` for an id that is not a provider's (`local` included), and as `setCredential` does where the app keeps no keys (`credentials_unavailable`) or for a malformed id (`provider_invalid`) |
| `refresh(provider)` | asks `provider` again now, past any listing kept, with the key the keychain has (what Test does); `catalogs()` then lists what it said. Rejects `catalog_not_found` |
| `capabilities()` | `{runs: "native", os: "macos"\|"windows"\|"linux", arch: "aarch64"\|"x86_64", has: ["cpu", …], memory_mb: number\|null}` — `has` is what sidevoice-engine runs a model on here (`cpu`; `metal` on Apple Silicon, for whisper.cpp), never Core ML; remote providers are catalogues of their own (`catalogs()`), `memory_mb` the machine's total, from the OS |
| `installed()` | `[{model, engine}]` — builds whose files are on disk and whole |
| `install(model, {build?, engine?, onProgress?, signal?})` | `ModelCatalogs.install`: downloads a build of the model, whatever of it is missing, and resolves `undefined`. `build` is the build's catalogue id (`ModelBuild.id`, `whisper-small/sherpa-onnx-int8`), `engine` its engine id (`backend`); either or both (below), or neither for the build the catalogue recommends here (`recommendedBuild`). `onProgress({fraction, done, total})` about twice a second, only with this call's own bytes (below). `signal`, an `AbortSignal`, cancels it, waiting or running: it rejects `{code: "install-cancelled", key: "install_cancelled", message}` and the model is not on disk afterwards (unless it already was before the install); a signal already aborted rejects so at once, and nothing reaches the app. Any other refusal rejects `{key, code, message, …params}` (below) |
| `uninstall(model)` | takes `model` off this computer: every build of it, and each of its files no other installed build shares (sidevoice-engine's `uninstall(model)`). Resolves `undefined`, also when none of it was on disk. Rejects with a refusal whose `code` the page reads (`failureCode`): `model_in_use` (`model-in-use`) while the voice call holds a build of it in memory, `model_installing` (`model-installing`) while an `install` of it is waiting or running — the page aborts that install first (its `signal`), the app never cancels it for it —, `model_unknown` (`model-not-found`) for a model the local catalogue does not list, `uninstall_failed` (a storage code) when the disk refused. It waits for an install of another model that is running to end, as installs wait for each other, and no install starts while it removes |
| `memory()` | `{total_mb, available_mb}`: the machine's memory and what is available of it now, each `null` when the OS does not say. On macOS `available_mb` is the share the kernel's memory-pressure level reports free (`kern.memorystatus_level`): a gauge, not a limit, since macOS compresses and swaps rather than fail |

- `model` is the catalogue id (`whisper-small`, `kokoro-82m-v1.0`), `engine` the build's engine id (`sherpa-onnx`,
  `whisper-cpp`): sidevoice-engine's model id and backend id. The build is the one `build` names (refused
  `build_missing` when the model has no such build, or when `engine` is named too and is not its engine); else, with
  only `engine`, that engine's build; else the recommended one. The app installs exactly that build or refuses it
  (below), and never another build instead. Before it downloads anything it checks, at this trust boundary, that the
  build runs here: an engine in this app, the build's needs and accelerators, and the model's memory against the
  machine's (unknown memory is not a refusal).
- Installs run one at a time. Each `install` call is its own job (`job`, an id the bridge makes); the app keeps
  progress per job from the moment the job starts — after any install ahead of it — until it ends, so a call that
  waits reports nothing and no call ever reports another's bytes. Each report is one object:

  ```json
  { "fraction": 0.082, "done": 52428800, "total": 639387718 }
  ```

  `done`/`total` are this job's bytes across the build's files; `total` comes from sidevoice-engine's catalogue, so it
  is known from the first report (0 when nothing is left to fetch, and then `fraction` is `null`). `fraction` is
  `done / total`, at most 1.
- A cancelled install stops within a moment, whatever it is doing: waiting for another install, waiting on the
  network — for the answer's headers or its next chunk, however long the server has gone quiet: the request is
  dropped and its connection closed — or unpacking. It frees the install lock, removes what it was downloading
  (sidevoice-engine stores a file only once it is whole and verified) and rejects with `install_cancelled`. A file it
  had already completed is whole and verified, and stays. A cancel the app accepted wins: one that lands as the files
  are moved into place removes the build again, and only it (another build of the model stays; a removal that fails
  rejects with `install_cancel_failed`), and a transport error that follows it (the connection it closed) is still
  `install_cancelled`, never `download_failed`. There is no overall download timeout: a slow line is not a failure,
  and the cancel is how a person stops one.
- Commands behind it (`src-tauri/src/engine_ipc.rs`): `engine_catalogs`, `engine_set_credential {provider, key}`,
  `engine_has_credential {provider}`, `engine_test_credential {provider, key}` (the reason with its `detail?`, or
  `null`), `engine_refresh {provider}`, `engine_capabilities`, `engine_installed`,
  `engine_install {model, build, engine, job}` (`build` and `engine` each a string or `null`) with
  `engine_progress {job}` (`{job, model, engine, done, total, bytes_per_s}` — the job's bytes and its speed, `null`
  until a second is measured — or `null` while the job waits or after it ends), `engine_cancel {job}` (what the
  signal's abort calls: `true` when the install will reject `install_cancelled`, also when it has not reached the app
  yet, which then refuses it as it arrives; `false` when it had already ended), `engine_uninstall {model}`,
  `engine_memory`. Granted to the room
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

The page renders by `key` and falls back to `message`. `host.engine`'s catalogue, key and install calls also carry
`code`, the code sidevoice-web's `ModelCatalogs` reads (`failureCode`): the refusal's own `code` (the engine's, below)
when it has one, else its `key` with dashes (`model-needs-memory`, `install-cancelled`). The engine's keys are made in `src-tauri/engine/src/error.rs`,
the voice call's in `src-tauri/src/voice.rs` and core `voice.rs`; a new one is added there and here. What
sidevoice-engine itself fails with is one of these keys, with the engine's own code beside it as `code`
(`digest-mismatch`, `model-load-failed`…).

| `key` | Parameters | When |
|---|---|---|
| `engine_unsupported` | `engine` | no runtime in this app for that engine: unknown, a page's engine, or one a newer catalogue added |
| `model_unknown` | `model` (and the engine's `code`, `model-not-found`, when the engine said it) | not in sidevoice-engine's catalogue |
| `build_missing` | `model`, `engine` | the model has no build for that engine, or none with the `build` id named (then `engine` is that id), or the build named is not the `engine` named |
| `build_unfit` | `model`, `engine`, `reason` (the engine's code, when it gave one) | the build needs a feature, or an accelerator, this device lacks |
| `model_needs_memory` | `model`, `needed_mb`, `memory_mb` | the model needs more memory than this machine has |
| `model_wrong_task` | `model`, `task` | the engine refused a model for what it was asked to do |
| `not_installed` | `model`, `engine` | the engine found a file missing |
| `download_failed` | `model`, `engine` | the network failed, or answered with an error |
| `download_corrupt` | `model`, `engine` | the bytes are not the ones the catalogue names (SHA-256) |
| `install_failed` | — | unpacking or moving the download into place failed (disk, permissions, an archive without its files) |
| `install_cancelled` | — | the page cancelled the install (its `signal`): not a failure, nothing to show as an error. `install` rejects it with `code: "install-cancelled"` |
| `install_cancel_failed` | `model`, `engine`, `code` | the page cancelled after the download had ended and removing it failed (`code`, the engine's: `model-in-use`, a storage failure): it may still be on disk |
| `model_installing` | `code` (`model-installing`), `model` | `uninstall(model)` while an install of that model is waiting or running: abort it first |
| `model_in_use` | `code` (`model-in-use`), `model` | `uninstall(model)` while the voice call holds a build of it in memory |
| `uninstall_failed` | `code` (the engine's: a storage failure), `model` | removing the model's files failed: some of it may still be on disk |
| `runtime_failed` | `engine` | the engine refused, with a code this app does not map |
| `voice_model_unknown` | `code` (`model-unknown`), `model` | a slot names a model its catalogue does not list for that task |
| `voice_model_unfit` | `code` (`model-unfit`), `model` | a local model with no build that runs on this device |
| `voice_catalog_not_found` | `code` (`catalog-not-found`), `model` | a slot names a catalogue the engine does not have |
| `voice_build_not_found` | `code` (`build-not-found`), `model` | a local slot's `build` is not a build of its model |
| `voice_<code>` | `code` (the engine's: `memory`, `backend-not-in-this-build`, ...), `model` | a local slot's `build` does not run on this device |
| `voice_vad_unavailable` | `code` (`vad-unavailable`) | no local voice activity detector runs on this device |
| `voice_end_of_turn_unavailable` | `code` (`end-of-turn-unavailable`) | `end_of_turn: "smart-turn"` while no local model that ends turns runs here |
| `voice_<code>` | `code` (the engine's: `credential-missing`, `credential-rejected`, ...), `model`, `detail` (the provider's words) | a slot names a provider's model while that provider lists nothing |
| `catalog_not_found` | `code`, `catalog` | `setCredential`, `testCredential` or `refresh` for a provider the engine does not have |
| `provider_invalid` | `provider` | not a provider id (lowercase letters, digits and dashes) |
| `credentials_unavailable` | — | `setCredential` or `testCredential` where the app keeps no keys (Windows, Linux) |
| `credentials_failed` | — | the keychain would not keep, remove or read a key |
| `voice_settings_missing` | `code` (`settings-missing`) | a call's `start` before its first `setSettings` |
| `voice_failed` | `code` (the call's: `microphone-denied`, `audio-device-unavailable`, the engine's `model-load-failed`, …, or `stopped`) | the call could not start, or was stopped before it listened |
| `engine_unavailable` | — | the engine did not start with the app |
| `update_unavailable` | — | `host.updater.restart()` with no update ready, `download()` with none known, or the `updater` command where this copy has no updater |
| `shortcut_invalid` | — | `host.muteShortcut.set(accelerator)` with one that is not a shortcut (below) |
| `bad_request` | — | a malformed call, or a caller other than the room window's own page: a bug in the caller |
| `internal` | — | something inside the app failed |

## The voice call

On macOS the app runs the voice host itself: sidevoice-voice (`sidevoice/sidevoice-voice`) with the device's own
microphone and speaker (cpal) and WebRTC's AEC3 between them, so the shared output cancels its echo
(`src-tauri/src/voice.rs`; sidevoice/sidevoice-core#89). sidevoice-voice names no model and depends on no engine:
the app fills its interfaces (voice activity, transcriber, speaker, end of turn) with the engine's models
(`src-tauri/src/voice/models.rs`). The room window's page is then never granted the microphone
(`media::decide`) and runs no model: it keeps the room. Each call knows nothing of the room: the page keeps one call per conversation, routes the shared microphone to one call, tags each call's events with its own destination, and has each call say what the room sends through a handle that tells how it went. Elsewhere `host.voice` is absent and the page runs the call
itself over `@sidevoice/voice` in the webview, the room window granting it the microphone: the page pinned now
(`web.pin.json`) cannot load those modules until they are on npm, so on Windows and Linux there is no voice yet.

### The seam

`window.__sidevoiceDesktop.host.voice` implements `VoiceHost`, the page's voice seam, defined once in sidevoice-voice:
[`js/voice-host.d.ts`](https://github.com/sidevoice/sidevoice-voice/blob/a15174b2d5eed09f1ba721d24090016a71868fbe/js/voice-host.d.ts) in
`@sidevoice/voice` (sidevoice/sidevoice-voice#6), with the types it carries (`VoiceSettings`, `VoiceHostError`,
and `js/voice-events.d.ts`'s `VoiceTurnEvent`, `VoiceCallState`, `VoiceSaying`, `VoiceSayOutcome`, `VoiceTake`) and what every
implementation promises
(that package's README, "The voice seam a page drives"). On the web the same package's `createVoiceHost(source)`
implements it over the call in the page, with the models the page's `source` loads (sidevoice-web's adapter over its
engine); the page does `host.voice ?? createVoiceHost(source)` and uses nothing else of either. It is not restated
here: a change is made there, and both sides follow.

What is this app's own:

- `setSettings` takes the web's settings (sidevoice-web `voice-settings.js`, `DeviceVoiceSettings`), each slot a model
  of a catalogue: `stt: {catalog, model, language, build?}`, `tts: {catalog, model, voice, speed, build?}`, with
  `patience`, `end_of_turn`, `idle_unload_minutes`, `barge_in_min_speech?` and `turn_model?` (core `voice.rs`;
  any other key is refused). `barge_in_min_speech` is a finite number of seconds greater than zero and at most 60;
  when absent, native voice uses 1.0 s. It changes the active call configuration without reloading models.
  A slot is loaded as `engine.catalog(catalog).load(model)`, which picks the build, or, for a local model with a
  `build` (Settings, "How it runs"; an empty one names none), as that build (`engine.load(model, build)`): once the
  model is found and runs here (else `model-unknown`, `model-unfit`), the build must be one of its builds
  (`build-not-found`) and run here (else the engine's code for why not, `memory`, ...). A provider's model has no
  builds, and a `build` named for one is not looked at, as on the web. The app adds the voice activity detector, the
  local catalogue's (family `silero-vad`, else another local `vad` that runs), and, for `smart-turn`, the local model
  that ends turns `turn_model` names when it runs here, else (as on the web) the first that does. Nothing stops the
  call (sidevoice-web #75, gap #31): the call holds one interface per slot (speech to text, text to speech with its
  voice, end of turn), and a change of a slot's catalogue, model or build, of `end_of_turn` or of the model that ends
  turns loads the new model while the one in use goes on serving, then moves the slot to it; what was under way (a
  turn being transcribed, a chunk of a reply being synthesized, a pause being asked) ends on the model it started with,
  and a reply's next chunks are the new speaker's, in its voice (as on the web). Only the latest change moves the
  slots: an earlier one still loading is dropped when it is done, and a later one never waits for it. Once the slots
  have moved, the configuration that goes with them applies (`smart-turn` once its model is in place), and
  `setSettings` resolves. `language`, `voice`, `speed`, `patience` and `idle_unload_minutes` apply live. The voice
  activity detector, the app's own choice, stays for the call: a different one comes with its next load. With no call
  holding models, the settings are what the next start loads.
  It rejects with the app's keyed refusals (above), each with the `code` the page translates (`model-unknown`,
  `model-unfit`, `catalog-not-found`, `build-not-found`, `vad-unavailable`, `end-of-turn-unavailable`, or the
  engine's own with the provider's `detail`), and with `{key: "voice_failed", code}` when the new model cannot be
  loaded: the call then goes on as it was, on the models it had.
- `start` rejects `{key: "voice_failed", code, message}` with the call's code, `{key: "voice_settings_missing", code:
  "settings-missing"}` before any settings, and `{key: "voice_failed", code: "stopped"}` when `stop` came first. A
  start after a stop starts the call again, whether or not that stop has resolved; a start while one is already asked
  or the call listens changes nothing (the first start's `take` stays).
- `stop()` resolves once the idle state that answers it has reached the page, after the take and the end of an open
  turn, and once every saying under way has its outcome; before the first `setSettings` it resolves at once.
- `mute(muted)` and `cancelInput()` return nothing and never reject, as in a browser. A call keeps its mute from its
  creation: one asked before the first `setSettings` applies to the call those settings make. A muted call hears
  silence; an open turn then ends only as silence ends it (its ordinary patience). `cancelInput()` before the settings
  does nothing.
- `endInput()` closes the call's open turn now with what was captured so far (sidevoice-voice `endInput`): it is
  transcribed and told like any turn (`finished`, or `cancelled` with `merged` when a turn opening within the merge
  window joins it). It resolves once the turn is closed, not transcribed, and never rejects: with nothing to close (no
  settings yet, no open turn, the call stopped or disposing) it resolves at once. It does not mute, nor touch the
  playback or how later turns end. It is asked in order with `mute`.
- The host has `createCall`, `routeInput`, `audioDevices`, `setAudioDevices`, and `onAudioDevices`.
  Every `VoiceCallHost` has `id`, `setSettings`, `start`, `stop`, `dispose`, `say`, `mute`,
  `cancelInput`, `endInput`, `onTurn`, `onState`, `onLevel`, `onPlaybackLevel`, `onError`, and `onTake`.
  `routeInput(call | null)` acknowledges a next-frame switch; the other started calls receive silence.
  `dispose` takes no new work, stops the call and delivers what that stop ends, as the call reports it: each saying's
  own outcome (never a made-up one), an open turn's `cancelled`, the take; then it releases only that call's lane and
  delivers nothing more. Stopping one call leaves the other calls and shared devices running.
  The final stopped lane closes the physical microphone and speaker. The catalogues and provider keys are
  `host.engine`'s (above).
- `onPlaybackLevel(listener)` receives a bare number from 0 to 1, just as `onLevel` does for the microphone: RMS of
  the samples actually rendered by the speaker, after fade and gain, smoothed on the voice detector's windows
  (normally 512 samples at 16 kHz, 32 ms). It continues while the microphone is muted. Silence and the end of
  playback deliver exact `0`, including after cancellation, failure and stop; stop sends zero before the terminal
  state or error. The voice module drops stale frames across stop/start and device replacement. Each subscription
  hears only later events, survives stop/start, isolates listener exceptions and returns an idempotent unsubscribe
  function that removes only that listener. Reload stops the call and discards the page's old subscriptions.
  The web UI feature-detects this method and wraps its bare number in its own
  `{threadId, level, receivedAt}` record for `window.sidevoiceUI.playbackLevel`; that record is not a bridge event.
- A try's take (sidevoice-web's contract gap #40): `start({take: true})` has the call keep what the microphone gives
  from the moment it listens until that listening ends, at most two minutes, in memory; `onTake` gets it then as
  `VoiceTake` (`{sampleRate, samples: Float32Array, started_at, ended_at, turns}`), before the error that ended the
  listening, and `stop()` after such a start resolves only once it was handed over (at the idle state that answers
  the stop, which the take comes before). A start with no take records nothing. The app keeps no copy.
- The audio devices are the system's (cpal's, each id cpal's device id), chosen through sidevoice-voice's
  `NativeDevices`; `outputSelectable` is always `true`. A call that runs switches its microphone at once and its
  speaker once what it is playing has ended. `setAudioDevices` rejects `{key: "voice_audio_device_unknown", code:
  "audio-device-unknown"}` for an id the system does not list. The system tells of no change, so from the first
  `audioDevices` or `setAudioDevices` the app lists the devices every 2 s and tells `onAudioDevices` each change
  (other devices, another choice, or a chosen one that went away, said once as `lost` and replaced by the default).
- A page that loads anew has no old calls or lanes. Loaded model instances are weakly cached while calls use them; the existing per-call idle period governs unloading.

### How it travels

- Page → app: `voice_create_call {key}`, `voice_route_input {key?}`, then per-call
  `voice_set_settings {key,settings}`, `voice_start {key,take?}`, `voice_stop {key}` (answers `true` once the page
  has what the stop ends, `false` at once before the first settings),
  `voice_dispose_call {key}` (stops first, the same way), `voice_say {key,sayKey,text,language?}`,
  `voice_cancel_say {key,sayKey}`, `voice_mute {key,muted}`, `voice_cancel_input {key}`, and
  `voice_end_input {key}` (answers once the turn is closed).
  The shared commands are `voice_audio_devices` and `voice_set_audio_devices {choice}`.
  The room window capability grants only these fixed commands, and each checks the current page.
- App → page: `window.__sidevoiceDesktop.voiceEvent(event)` receives `{key,type,data}`.
  `key` is the immutable call ID captured before asynchronous native forwarding. `type` is
  `turn`, `state`, `level`, `playback-level`, `error`, `take`, or `say`.
  `say` uses `{key,type:"say",data:{key:sayKey,event}}`; its last event is `done` with the terminal outcome.
  Device changes use `{type:"devices",data}` and have no call key. Take samples are base64 on the native
  wire and become `Float32Array` in the bridge.
- A turn's `started`, `finished`, and `cancelled` retain their voice fields and order within their
  source call. The native bridge never reads the current route when forwarding them. The page attaches its
  conversation destination and captured binding to that call's events, including a delayed `started` that
  arrives after another call becomes routed. STT and merge state stay with the source call; silence to an
  unrouted call ends an open turn only under its normal VAD and patience rules. A quick return can continue
  that call's turn. No app destination is added to voice's event type.
- Rust serializes `StopReason` inside a `reason` object; the JS bridge flattens both saying `done`
  and `outcome` to `{status:"heard"}`,
  `{status:"heard-up-to",heard_chars,reason:"barge-in"|"cancelled"|"stopped"}`, or
  `{status:"not-played",reason:"barge-in"|"cancelled"|"stopped"}`. Failure carries
  `reason:"failed",code`. A `cancel` follows its `voice_say` request. The handle's `done`/`outcome`
  is the sole physical terminal report, including a barge-in cut. The web layer owns reply hearing,
  skip policy, and core publication IDs.
- `voice_mute {key,muted:true}` feeds that call silence without muting its speaker.
  The app mirrors a global user mute to all calls. Native output from every call is mixed once before
  the single AEC reference. Each call's playback level reflects only its own rendered samples; a device
  failure is sent to each affected call.
- Keys: `host.engine.setCredential` writes the macOS keychain (`src-tauri/src/keychain.rs`), and the engine reads it
  when a provider is listed, or its model loaded or called. `host.engine.testCredential` writes nothing: its key goes
  only to a trial engine of its own.

## The microphone permission

Where the app runs the voice call (`host.voice`, macOS), the room page is never granted `getUserMedia`, so it can
neither trigger macOS's microphone prompt (TCC) nor read its answer. `window.__sidevoiceDesktop.host.microphone` does
it for the page, on the app's own privacy entry, the one the voice call uses: the web's first-run setup asks for the
microphone early through it, rather than at the first call. Present exactly where `host.voice` is; a frozen object:

```ts
interface MicrophonePermission {
  state(): Promise<"prompt" | "granted" | "denied" | "missing">;
  request(): Promise<"granted" | "denied">;
  openSettings(): Promise<void>;
  onChange(listener: () => void): () => void;
}
```

- `state()`: `AVCaptureDevice.authorizationStatus(for: .audio)`: not determined → `"prompt"`, authorized →
  `"granted"`, denied or restricted → `"denied"`. Where it is not denied and no audio input device exists
  (`AVCaptureDevice.default(for: .audio)` is nil), `"missing"`: the OS knows the devices, so the bridge owns device
  availability, and a denial stands whatever is plugged in. Never prompts.
- `onChange(listener)`: `listener()` whenever `state()` changes, so on a permission change and on a microphone
  plugged in or out; the page reads `state()` again. Polled every 2 s while anyone listens; returns `stop`.
- `request()`: `AVCaptureDevice.requestAccess(for: .audio)`: macOS's own prompt the first time (with
  `NSMicrophoneUsageDescription`, `src-tauri/Info.plist`); afterwards it answers at once with the stored answer, no
  prompt.
- `openSettings()`: opens System Settings at Privacy & Security › Microphone
  (`x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone`, through `/usr/bin/open`), where a
  denial is undone.
- A refusal rejects `{key, message}`: `bad_request` for a malformed call or another caller, `internal` when the system
  does not answer or System Settings does not open.
- Command behind it: `microphone {action}`, `action` `state`, `request` or `open-settings` (`src-tauri/src/microphone.rs`),
  macOS only. Granted to the room window (`capabilities/room.json`); it re-checks that the caller is the current room
  window on the app's own page.

## The mute shortcut

The global shortcut that mutes and unmutes the microphone (docs/SHORTCUTS.md), set from the web's Settings. Always
there.

```ts
interface MuteShortcutBridge {
  get(): Promise<MuteShortcut>;
  set(accelerator: string | null): Promise<MuteShortcut>;   // null: none
  subscribe?(listener: (shortcut: MuteShortcut) => void): () => void;
}
interface MuteShortcut {
  accelerator: string | null;   // as the app keeps it: "Ctrl+Alt+M"; null when there is none
  label: string | null;         // as this system writes it: "⌃⌥M" on macOS, "Ctrl+Alt+M" elsewhere
  state: "on" | "off" | "refused";  // refused: the system would not take it (another app may hold it)
  alternatives: string[];       // refused: accelerators the system does take, to offer instead (set one as it is)
  voiceOver: boolean;           // macOS: VoiceOver is on and the shortcut holds Control and Option together
}
```

- `set` takes the web's form as it is: `+`-separated, any case, one or more of the modifiers `Cmd`/`Command`,
  `Ctrl`/`Control`, `Alt`/`Option`, `Shift`, then one key as the browser names it (`event.key`: `M`, `7`, `F5`, `Space`,
  `ArrowUp`, `;`, `+`). With Shift, a character Shift makes on a US keyboard is read as its key (`Ctrl+Shift+?` is
  Ctrl+Shift+/, `Cmd+Shift++` is Cmd+Shift+=). The app keeps it in one form,
  modifiers in the order `Cmd+Ctrl+Alt+Shift` and the key's first letter upper case (`shift+cmd+m` → `Cmd+Shift+M`;
  core `settings::normalize_shortcut`), and answers it as `accelerator`. `Cmd` is the Command key (Super elsewhere).
  Anything else — no modifier, no key or two, a character no key makes (`µ` from Option+M on a Mac, a dead key) —
  rejects `{key: "shortcut_invalid", message}` and changes nothing. One the system refuses is kept, and answered `refused`.
- Kept with the app's settings (`muteShortcut`), set only from the page.
- App → page: `window.__sidevoiceDesktop.muteShortcutEvent(shortcut)` (`webview.eval`) whenever the app registers it
  (set from the page), to `subscribe`'s listeners. `voiceOver` is read when asked: VoiceOver
  turned on or off tells no one.
- Command behind it: `mute_shortcut {action, accelerator?}`, `action` `get` or `set`.

## The call controls settings

Whether the call controls card shows during a call always, also while the room's window is in front, or only while it is
behind or minimised ("Always show the call controls", above), set from the web's Settings. Always there.

```ts
interface CallControlsBridge {
  settings(): Promise<{ alwaysShown: boolean }>;            // default false
  setAlwaysShown(enabled: boolean): Promise<{ alwaysShown: boolean }>;
}
```

- `setAlwaysShown` keeps it with the app's settings (`callControlsAlwaysShown`) and applies it to the card at once; a
  non-boolean rejects `{key: "bad_request"}`. Set only from the page.
- Command behind it: `call_controls_settings {action, enabled?}`, `action` `settings` or `set-always-shown`.

Both commands are granted to the room window (`capabilities/room.json`) and re-check that the caller is the current
room window on the app's own page (else `bad_request`). The app's settings (`settings.json`) have one writer
(`change_settings`, `src-tauri/src/lib.rs`): every setter of the page changes only its own fields, under one lock, so
none loses another's. The app's settings window saves nothing; it only shows diagnostics.

## The build

`window.__sidevoiceDesktop.host.build` is what this build of the app is, decided when it was built (RELEASING.md →
"Channels"; `src-tauri/build.rs`, core `build_info`), so the page can say it:

```ts
{ channel: "release" | "nightly" | "dev"; version: string; commit?: string; date?: string }
```

`release`: a numbered release, with `host.updater` on the releases' feed. `nightly`: the rolling build of `main`, with
`host.updater` on the nightly's feed; `version` is its own (`X.Y.Z-nightly.N`, RELEASING.md → "Channels"), with the
short `commit` and that commit's `date` (`YYYY-MM-DD`).
`dev`: anything else (a laptop, a pull request, a manual run). Always present.

## The updater

The app's own updates (not the service's or the connector's). A release and a nightly have it, each on its own
channel (RELEASING.md → "Updates"): a release reads the releases' feed and is offered only a newer release; a nightly
reads the nightly's feed and is offered only a newer nightly, also after a release ships (moving to the other channel
means installing its build by hand). The feed is the build's channel's (core `updater::feed`), never a setting. And
only where that copy can replace itself: its `.app` run from its bundle (not translocated, not from the .dmg), the
AppImage run as one, the Windows installer's install. Elsewhere (a dev build, a `.deb`, a bare binary) the page has no
`host.updater`, nothing is checked, and the commands refuse `update_unavailable`.

```ts
interface AppUpdater {
  settings(): Promise<{ mode: "automatic" | "manual" }>;            // default "automatic"; kept by the app
  setMode(mode: "automatic" | "manual"): Promise<{ mode: "automatic" | "manual" }>;
  status(): Promise<UpdaterStatus>;
  subscribe(listener: (status: UpdaterStatus) => void): () => void;  // every change
  check(): Promise<UpdaterStatus>;                                    // resolves after the check
  download(): Promise<UpdaterStatus>;                                 // resolves once it starts
  cancel(): Promise<void>;                                            // stops a download in progress; resolves too with none
  restart(): Promise<void>;                                           // only when "ready": restarts now into the update
}
interface UpdaterStatus {
  state: "idle" | "available" | "downloading" | "ready";
  version?: string;            // the update's (available/downloading/ready)
  progress?: number;           // 0..1 while downloading, in hundredths (absent while the size is unknown)
  error?: { code: "update-check-failed" | "update-download-failed" | "update-install-failed" }; // last attempt; state unchanged
}
```

- **Automatic** (the default): checked at launch and then every hour (a nightly every 10 minutes), an update found is
  downloaded and verified in the background → `ready`; the page offers "Restart to update". An update ready when the
  person quits the app (tray, ⌘Q) is installed on the way out (Windows: its installer runs without starting the app
  again). The app never restarts on its own.
- **Manual**: checked only → `available` → `download()` → `downloading` → `ready`.
- `restart()` installs the ready update and restarts the app into it, now; it knows nothing of calls (the person
  decides when). Rejects `{key: "update_unavailable", message}` unless the state is `ready`; `download()` likewise
  when no update is known. An update already downloading or ready is left as it is by later checks (nothing is
  downloaded twice); one still `available`, found again, is not told to the page again (only an `error` the check
  clears is).
- `cancel()` aborts the download running and drops what it had received (the plugin downloads into memory: there is no
  partial file); the state goes back to `available` (the update stays known, no `error`). With no download running it
  resolves and changes nothing. The cancelled version is remembered for this run of the app (not with the settings):
  automatic mode does not download it again by itself, at later checks or when it is switched to; a newer version found
  is downloaded as usual, and `download()` (the person asking) lifts the cancel. A new launch starts clean.
  Each download holds a token that `cancel()` invalidates at once: an aborted download still finishing its last step
  cannot write progress, bytes or a result over a download that replaced it.
- A check that fails or takes over 2 minutes, a download that fails or takes over an hour, an install that fails:
  `error` says so, the state stays (a failed download stays `available`, and automatic mode tries it again at the
  next check).
- The mode is kept with the app's settings (`updateMode`), set only through `setMode`.
- App → page: `window.__sidevoiceDesktop.updaterEvent(status)` (`webview.eval`) on each change, to the listeners.
- Command behind it: `updater {action, mode?}`, `action` one of `settings`, `set-mode`, `status`, `check`,
  `download`, `cancel`, `restart`. Granted to the room window (`capabilities/room.json`); it re-checks that the caller is the
  current room window on the app's own page (else `bad_request`). The page is granted none of the plugin's own
  commands. `src-tauri/src/updater.rs` drives tauri-plugin-updater (check, download with its progress, verify,
  install); core `updater.rs` holds the status and the mode (unit-tested). CI's macOS job checks the
  probe app (a dev build) has `host.build.channel` `"dev"` and no `host.updater`.

## Changing the bridge

- Add a command: a variant in `Command` (Rust), a `case` in `run` (JS), a test on each side.
- Add a field: `CallSnapshot` uses `#[serde(default)]`, so old and new scripts interoperate;
  bump `version` only for a breaking change.
- Tests: `npm test` (the script against a fake window, `host.voice`, `host.microphone`, `host.updater`, `host.build`, `host.muteShortcut`,
  `host.callControls` and `host.localHost`'s install included), `cargo test -p sidevoice-desktop-core` (the voice call's builds, the media rule, the
  updater's status and the mute shortcut's form and shape among the rest) and `cargo test -p sidevoice-desktop-engine` (the engine
  over a test host), `cargo test -p sidevoice-local-host` (the local host against a stand-in core, its install against a stand-in
  registry; docs/LOCAL_HOST.md → Tests). In the real app, CI's macOS job runs the probe page (`test/fixtures/probe.html`, probe build): the engine's
  capabilities, installs, refusals and a cancelled download (CI then checks nothing of it is on disk), and the voice
  call's seam, catalogue, builds and a start that settles. CI's macOS job also runs the app's `voice::` tests: the
  native host's lanes through the commands' own methods over stand-in shared devices (two started lanes, the route
  to one, the other, none; a device failure on one; dispose releasing one lane and the last closing the devices; a
  mute before the settings; start after stop; dispose's delivered ends), and a call on the engine's real models
  through the app's adapter. The shared audio itself (`SharedNativeAudio`: one capture, silence to unrouted lanes,
  the mixed echo reference) is sidevoice-voice's, tested in its own CI.
