# Global shortcuts and headset controls (design)

Status: draft for the operator's review (2026-11-05). Nothing here is implemented. Nothing was tried on a real machine.

Labels used throughout: **Verified** means read in this repository's code or docs, or in the cited primary source, on
2026-11-05. **Inference** means reasoned from those facts and not checked. **Not checked** says what could not be read.

Every user-facing text proposed here is an i18n key in the English bundle (the web's `en.ts`, the fallback), per
`AGENTS.md`. Other languages may lag. No literal sentence is meant to ship.

## 1. What exists today

### 1.1 In the room page (sidevoice-web, in-app keys)

Read from `apps/web/src/state/shortcuts.ts`, `apps/web/src/state/use-room-shortcuts.ts` and
`apps/web/src/services/room-session-controller.js` (worktree `stage-polish`, at `v0.23.0-2-gfa667d45`; desktop pins
`v0.23.0`, `web.pin.json`). **Verified.**

| Action | Keys | Where it lives | Behaviour |
|---|---|---|---|
| Mute toggle | Cmd+D (macOS), Ctrl+D (elsewhere) | `shortcuts.ts` id `mute`; handler in `use-room-shortcuts.ts` | `window.sidevoiceActions.toggleMic()`. Ignores key repeat, ignored with Alt held and while typing in a field or menu. |
| Hold to talk | Space (physical `Space` code, no modifier) | `shortcuts.ts` id `holdToTalk`, `hold: true`; handler `room-session-controller.js` (`HOLD_KEY`, around line 2952) | It is "talk while muted". On keydown, if the mic is **muted**, it opens it and sets `holding`. On keyup it mutes again. If the mic was already open, Space does nothing. It is only active when `voice` is on. The English label is "Talk while muted" (`settings.shortcuts.holdToTalk`). |
| Skip reply | Cmd/Ctrl+S | `shortcuts.ts` id `skip` | `skipReply()`. The browser's Save is suppressed. |

Two facts that matter for global use:

- Hold-to-talk **releases on page `blur` and on `visibilitychange` to hidden** (`window.addEventListener('blur', releaseHold)`).
  A key held while Sidevoice is out of focus is exactly the global case, so the page's own handler cannot be reused as is.
  The page needs a host command pair that sets and clears `holding` without relying on focus (section 7). **Inference** on
  how the webview behaves when hidden or occluded (timers may be throttled); it needs a device test.
- The web's Settings > Shortcuts section is **hidden**: `useShortcutsSection()` returns `false`
  (`ShortcutsSettings.tsx`, operator decision 2026-10-16, web#148, "until shortcuts are redesigned and customizable").
  Nothing else renders `MuteShortcutRow`, so **today a person cannot change the global mute shortcut from the UI**
  (the default `Ctrl+Alt+M` is registered and works; the editor is not reachable). **Verified** in the `stage-polish`
  worktree; the pinned `v0.23.0` was not diffed.

### 1.2 In the desktop app (a global mute shortcut already exists)

- **One global shortcut, mute toggle.** `DEFAULT_MUTE_SHORTCUT = "Ctrl+Alt+M"` (shown as ⌃⌥M on macOS)
  (`desktop/core/src/settings.rs:15`), stored as `Settings.mute_shortcut: String`, editable only through the bridge
  `host.muteShortcut` (`docs/BRIDGE.md`, "The mute shortcut"; `desktop/tauri/js/desktop-bridge.js` `muteShortcut()`;
  command `mute_shortcut {action, accelerator?}` in `desktop/tauri/src/lib.rs`).
- **Mechanism.** `tauri-plugin-global-shortcut` 2.4.0 (`desktop/tauri/Cargo.toml`, `Cargo.lock`), which uses the
  `global-hotkey` crate 0.8.0 (`Cargo.lock`). Registration and the handler are in
  `desktop/tauri/src/platform/shortcut_access.rs`. The handler **ignores `Released`**:
  `if event.state() == ShortcutState::Pressed { send(Command::ToggleMute) }` (line 52). `replace()` does
  `unregister_all()` then registers the one accelerator, so the code holds exactly one shortcut today.
- **Registration state.** `Registration` is `Off | On | Refused | Unavailable` (`settings.rs:170`). `Refused` means the
  system would not take it (another app may hold it); the app then probes `SHORTCUT_ALTERNATIVES`
  (`Ctrl+Alt+Shift+M`, then `Ctrl+Alt+Shift+K`, `settings.rs:63`) and answers the ones the system accepts.
  `Unavailable` means no backend: **native Wayland**, checked as GTK's real display type, not `DISPLAY`
  (`shortcut_access/linux.rs`).
- **macOS VoiceOver.** `voiceOver: true` when VoiceOver is on and the chord holds Control and Option together
  (`shortcut_access/macos.rs`, `docs/SHORTCUTS.md`).
- **Command path.** The shortcut sends `Command::ToggleMute` (`desktop/core/src/bridge.rs:237`), run on the room page
  as `window.__sidevoiceHost.run({command:"toggle-mute"})`, which calls `actions.toggleMic()`
  (`shared/tauri/js/members.js`, lines 479-482). The command set is `toggle-mute`, `hang-up`, `skip-reply`,
  `select-participant`, `select-audio-device`, `open-settings`, `show-settings`, `cancel-input`, `keep-alive`.
  **There is no hold or press-and-release command.**
- **Known open points already in the repo.** The default and its alternatives are the operator's decision (desktop#10,
  `docs/SHORTCUTS.md`). `Ctrl+Alt+<letter>` is AltGr on many Windows layouts (µ, §, W, Ķ; `docs/SHORTCUTS.md`).
  Runtime verification on macOS, Windows and Linux X11/Wayland is pending; the shortcut code was only
  cross-compiled and unit-tested.
- **Type drift.** The desktop answers `state: "on" | "off" | "refused" | "unavailable"` (`docs/BRIDGE.md`), but the web's
  `MuteShortcut.state` is `"on" | "off" | "refused"` (`apps/web/src/host/types.ts:287`). The page's `Keys` row
  gets an `unavailable` it does not name. **Verified.**
- **Docs that mention it.** `docs/SHORTCUTS.md`, `docs/BRIDGE.md`, `docs/BRAND.md` (line 115), `docs/MACOS.md`
  (line 105, mobile/phone has no tray or global shortcut), `docs/MOBILE.md` (line 82, `muteShortcut` is not offered on
  the phone), `THIRD_PARTY_NOTICES.md`.

### 1.3 Headset and media buttons (already supported)

`docs/MEDIA_CONTROLS.md`, `shared/tauri/src/headset.rs`, `shared/tauri/src/platform/media_controls/*`,
`mobile/tauri/plugin/...`. **Verified.**

- One rule for every host (`headset.rs`, `media_command_action`): `play` unmutes (and does nothing if already open),
  `pause` and `toggle` toggle mute. There is **no next/previous mapping** on any host
  (`docs/MEDIA_CONTROLS.md`: "adding one is its own decision"). A hang-up gesture is "not claimed".
- **macOS:** MediaPlayer `MPRemoteCommandCenter` (play, pause, togglePlayPause) plus the AirPods mute gesture through
  `AVAudioApplication.setInputMuteStateChangeHandler` (`media_controls/apple.rs`, `macos.rs`). The app is the Now
  Playing app only while a call is on, and offers `presence: { media: "app" }`.
- **Windows and Linux:** no native owner (`media_controls/linux.rs` is an empty `Os` impl). The page's own Media
  Session is the fallback; whether a physical button reaches it is "unverified in WebView2/WebKitGTK".
  Native SMTC and MPRIS are tracked in desktop#196.
- **iOS:** two modes. While the call is a CallKit call (a **nightly developer option, off by default**:
  D-NO-CALLKIT-DEFAULT, `docs/MOBILE.md`), the headset's buttons and the AirPods gesture act through CallKit
  (`CXSetMutedCallAction`, `ios.rs:540` `perform_mute`). Otherwise MediaPlayer remote commands (play, pause, toggle),
  registered **only while Now Playing shows the call**, and let go when the Live Activity runs or the call ends,
  because a handler registered too early leaves an empty Lock Screen card. AirPods mute gesture on iOS 17+ through
  `AVAudioApplicationInputMuteStateChangeNotification`.
- **Android:** a `MediaSession` owned by the call's foreground service (`CallService.kt:599-631`), callback
  `onPlay`/`onPause`/hang-up custom action only. State is `PLAYING` while the mic is open, `PAUSED` while muted, so
  a single headset press arrives as pause then play. The session exists only during a call.
- **Not claimed anywhere:** push-to-talk from a headset, double or triple press, long press, outside-of-call behaviour.
  All physical-button behaviour is "unverified on hardware".

## 2. Goals and actions

| # | Action | Semantics | Status |
|---|---|---|---|
| A1 | **Hold to talk** (global) | Press: if muted, open the mic. Release: end the turn and mute again, identical to the in-app Space (it does nothing if the mic was already open). | New |
| A2 | **Mute toggle** (global) | Same as Cmd/Ctrl+D. | Exists as `Ctrl+Alt+M`; reuse and extend (section 7) |
| A3 | **Say one thing** (later) | Press once: the mic opens for one turn; when the turn ends (the existing end-of-speech detection), it is sent and the mic mutes. | Future; needs a page-side hook (section 7) |
| A4 | **Headset** | Map the buttons a headset can send to A2, A3, skip reply, or nothing. | Partly exists (play/pause toggles) |

A1 ends "the turn" by muting. Whether release also sends immediately, or lets the existing silence detection finish the
turn, is a question for the page (open question 7). The in-app Space only mutes.

## 3. Per-OS table (desktop global shortcuts)

| | macOS | Windows | Linux X11 | Linux Wayland |
|---|---|---|---|---|
| **Mechanism today** | `global-hotkey` 0.8.0 on Carbon `RegisterEventHotKey` + `InstallEventHandler` | `global-hotkey` on `RegisterHotKey` (with `MOD_NOREPEAT`) | `global-hotkey` on `XGrabKey` (x11rb), key events from the X connection | none: `unavailable` |
| **Release detected?** | Yes. The crate installs both `kEventHotKeyPressed` and `kEventHotKeyReleased` and emits `Pressed`/`Released` (source, 0.8.0 tag). | Yes, but by polling. `WM_HOTKEY` only fires on press; the crate then spawns a thread that polls `GetAsyncKeyState` every 50 ms and sends `Released` when the key is up (`platform_impl/windows/mod.rs`; the 50 ms sleep is in the 0.8.0 changelog). Release latency up to about 50 ms. | Yes, from `KeyRelease` events on the grab (`platform_impl/x11/mod.rs`); an old bug where release was missed when modifiers were let go before the key is fixed (changelog, #47). | Portal `Deactivated` signal (see below). |
| **Permission** | None for `RegisterEventHotKey` on a key plus modifiers. **Inference** from Apple's model: a shortcut registered through the hot-key API sees only its own combination, so it needs neither Input Monitoring nor Accessibility. Apple's own pages for the API could not be read (below). | None. | None. | Consent through the portal's dialog (`BindShortcuts`), once per app, and the user may change the trigger. |
| **Modifier-only or Fn as the key** | Not possible with Carbon. Needs a `CGEventTap` on `flagsChanged`: a listen-only tap needs **Input Monitoring**, an active (default) tap needs **Accessibility** (**Inference**, Apple's tap page returned no body text; Apple's [Input Monitoring page](https://support.apple.com/guide/mac-help/control-access-to-input-monitoring-on-mac-mchl4cedafb6/mac) confirms the privacy pane, not the API mapping). | Needs a `WH_KEYBOARD_LL` hook: **no permission**, sees every key including key-up (Microsoft: `WM_KEYUP`/`WM_SYSKEYUP` delivered). Costs below. | Possible by reading `/dev/input` (needs the `input` group) or XInput2 raw events; not recommended. | Not possible. |
| **Where it breaks** | Option-only and Option+Shift-only hotkeys failed with error -9868 on macOS 15.0 for sandboxed apps; Apple said it was deliberate, and 15.2 beta 2 restored it, with mixed reports afterwards (developer forum thread and blog below). Another app holding the combo makes registration fail. VoiceOver keys (`Ctrl+Option`). Secure input (a password field, a terminal's secure keyboard entry): **Inference** that Carbon hot keys still fire; not checked. | Fails if another app registered the same combo (documented). `Alt+Space` is the window menu (**Inference**: registration may succeed but it hides a system behaviour; not checked). `Win`-key combos are reserved. `Ctrl+Alt+letter` is AltGr on many layouts (`docs/SHORTCUTS.md`). F12 is reserved for debuggers. Elevated windows (UIPI) and games that use raw input or exclusive fullscreen may not pass the key to `RegisterHotKey` (**Inference**, not checked). | The grab is exact: the crate also grabs the NumLock/CapsLock variants. Fails if another client holds the grab. A window manager or desktop may hold the combo first. | The app cannot know or force the final trigger: the portal returns a `trigger_description` string only and the user can rebind. |

Linux Wayland, in detail (the XDG `GlobalShortcuts` portal):

- Methods `CreateSession`, `BindShortcuts`, `ListShortcuts`; signals `Activated`, `Deactivated`, `ShortcutsChanged`.
  `Deactivated` is the release. `BindShortcuts` "will typically result in the portal presenting a dialog showing the
  shortcuts and allowing users to configure the shortcuts" and an app can only attempt to bind a session's shortcuts once
  (portal doc, verified).
- `preferred_trigger` is a preference; the app only gets back a readable `trigger_description`, so the settings row
  **shows text the portal gives, and cannot record a combo itself**.
- Compositor support is **not** verified from a primary source. A web search found KDE Plasma and Hyprland
  (`xdg-desktop-portal-hyprland`, no dialog and no default binding, the user binds by hand) and a secondhand claim that
  GNOME's portal landed around GNOME 48. wlroots-based portals and others did not appear to implement it. Treat support
  as "KDE yes, GNOME probably, others no" until tested.
- Neither `global-hotkey` 0.8.0 nor `tauri-plugin-global-shortcut` 2.4.0 has a portal backend (the docs.rs page lists
  "Linux (X11 Only)"). A portal client is separate code: **Inference**: the `ashpd` crate is the usual Rust client; it was
  not evaluated here and is a candidate for a `lib-researcher` pass.

Windows low-level hook, in detail (if modifier-only or Fn-like keys are ever wanted): runs on the installing thread, which
needs a message loop; must return fast: past `LowLevelHooksTimeout` (at most 1000 ms) "on Windows 7 and later, the hook is
silently removed without being called", with no notice to the app. Microsoft advises raw input where possible. A keyboard
hook is also what keyloggers use, so antivirus heuristics and reviews pay attention to it (**Inference**).

**Versions and Released events (verified).** `global-hotkey` added `Pressed`/`Released` state in changelog entry
`53961a1` (#35); 0.8.0 and the locked version emit it on macOS, Windows and X11. `tauri-plugin-global-shortcut` exposes it as
`ShortcutState::{Pressed, Released}` since the pressed/released refactor (`#1244`, plugin changelog); the repo locks
2.4.0, 2.4.1 exists (lock fix for a handler deadlock). `global-hotkey` 0.9.0 is released (`keyboard-types` 0.8, more keys). The
Tauri JS API is not needed: the repo registers from Rust. **No dependency change is needed for hold-to-talk on macOS, Windows
and X11.**

## 4. Permission strategy: ask only when needed

Principle: the app never asks at startup, never asks for a permission it could avoid by choosing a different mechanism,
and always asks from a row the person chose to configure, with a sentence saying why.

| Choice | Prompt? | Used for |
|---|---|---|
| macOS `RegisterEventHotKey` (today's path), a combination with at least one modifier | **No prompt.** (Inference; not tried.) | A1 and A2 as combinations. |
| macOS listen-only `CGEventTap` | **Input Monitoring** (appears in Privacy & Security; the first tap attempt triggers the system prompt, **Inference**) | Single modifier key or Fn as hold-to-talk, and nothing else. |
| macOS active `CGEventTap` | **Accessibility** | Never: not needed. |
| macOS media keys (`global-hotkey` creates a session `CGEventTap` with the default option for them, `start_watching_media_keys`) | Needs the tap permission | **Do not register media keys through `global-hotkey`.** Headset buttons go through `MPRemoteCommandCenter`, which needs none (section 5). |
| Windows `RegisterHotKey` | No | A1 and A2. |
| Windows `WH_KEYBOARD_LL` | No | Only for modifier-only keys; behind a clearly marked "advanced" choice. |
| Linux X11 `XGrabKey` | No | A1 and A2. |
| Wayland portal `BindShortcuts` | A system dialog, the portal's, the first time | A1 and A2, each registered as a portal shortcut. |
| `/dev/input` (Linux) | A group membership, not an in-app prompt | Not offered. |

When the ask happens:

1. **Never at launch, never at first call.** A combination shortcut needs none.
2. **On Wayland:** when the person first sets or enables a global shortcut (the row's Set button), the app calls
   `CreateSession` and `BindShortcuts`; the compositor shows its dialog. If it is dismissed or unsupported, the row
   shows the `unavailable` state with the in-app keys as the working fallback.
3. **On macOS, only if** the person picks a single-key trigger (Fn, Right Option, a lone modifier) for hold-to-talk. The app
   calls `CGPreflightListenEventAccess`, shows the permission row (Allow), and only on Allow triggers the system prompt
   (`CGRequestListenEventAccess`, **Inference**: the function names are from memory; Apple's page could not be read).
   If denied, the row says so and the trigger stays off; it does not fall back silently to another key.
4. **The status is read, not asked, when the page opens Settings** (the existing `muteShortcut.get()` pattern).

This follows the repo's existing permission rows, where reading status never prompts and a request is a separate call
(the microphone status and `request()` in `docs/BRIDGE.md`).

## 5. Headset buttons per platform

What a headset can send is decided by the OS, not by the app. What Sidevoice can see:

| Platform | API | What reaches the app | Press and release? |
|---|---|---|---|
| iOS, not a CallKit call | `MPRemoteCommandCenter`: play, pause, togglePlayPause, next, previous (`apple.rs:164`) | One event per click. Next/previous for double/triple press where the headset sends them (**Inference**). The app registers play/pause/toggle only. | No, a click is one command. |
| iOS, CallKit call | `CXSetMutedCallAction`, `CXEndCallAction`, `CXSetHeldCallAction` (`ios.rs:485-573`) | The headset button mutes, ends or holds the call as the system defines. | No. |
| iOS, AirPods stem gesture in a call | `AVAudioApplication` mute state change (iOS 17+, `apple.rs`) | A mute/unmute request the app keeps in step. | No. |
| Android | `MediaSession.Callback` (`onPlay`, `onPause`, and `onMediaButtonEvent(Intent)` not overridden today). `FLAG_HANDLES_MEDIA_BUTTONS` is deprecated and "no longer used" since API 26: every session handles media buttons. | Play/pause. Overriding `onMediaButtonEvent` would expose the raw `KeyEvent` with `KEYCODE_HEADSETHOOK` / `KEYCODE_MEDIA_PLAY_PAUSE` and `ACTION_DOWN`/`ACTION_UP`, which could allow press-and-release (**Inference**: Android KeyEvent has both; whether Bluetooth headsets send separate down and up, or only a click, varies by device and was not checked). | Unverified. Do not promise it. |
| macOS | `MPRemoteCommandCenter` for the Now Playing app; AirPods gesture via `AVAudioApplication` | As iOS. | No. |
| Windows | SMTC (`SystemMediaTransportControls`) | **Not implemented**; the page's Media Session is the fallback and unverified (desktop#196). | No. |
| Linux | MPRIS (D-Bus) | **Not implemented**; same. | No. |

Conclusions (inference, flagged):

- **Hold-to-talk from a headset is not a promise any platform makes.** The honest set is: press, and where the headset
  sends it, double and triple press. Put A1 on keyboard only; A2, A3 and skip on headset presses.
- **Outside a call, claiming the buttons has a cost the repo already measured.** On iOS a handler registered when no
  call is on makes the app the Now Playing app for the rest of the process, an empty Lock Screen card
  (`docs/MEDIA_CONTROLS.md`). On Android the session exists only with the call's foreground service. So "a press can join
  the call" outside a call would need the app to hold a media session all the time. Recommended default: leave the buttons
  to other apps outside a call.
- **The AirPods stem and Bluetooth "call" button** during a call, in the CallKit path, already act as the system's mute
  and end; in the default non-CallKit path the button is play/pause, and the system, not the app, decides what a squeeze
  sends. CallKit's own mute is the more reliable route, but the call-as-system-call is a nightly option the operator
  turned off by default on 2026-11-02, so do not couple this work to it.
- **Desktop buttons:** keyboard media keys (Play/Pause) on Windows and Linux depend on the page's Media Session
  today; a native SMTC/MPRIS owner is desktop#196. Do not register media keys with `global-hotkey` on macOS (section 4).

## 6. The settings UI

The operator's mockup is `/workspace/sidevoice/scratch/shortcuts-mock/index.html` (screenshot `shot.png`). Its groups:
"Anywhere on your computer", a permission row only when needed, "Headset buttons", "In Sidevoice". **The proposal below
follows that structure.** The page is the web's Settings > Shortcuts, which must be un-hidden
(`useShortcutsSection`; web#148). Keys are not real strings: the final text is in the English bundle first.

Where research suggests something different from the mockup is marked **Differs**.

### 6.1 Group "Anywhere on your computer" (`settings.shortcuts.global`, desktop only)

Hidden entirely on the phone (no `muteShortcut` bridge, as now). One row per action, each a `ShortcutRow`:
the keys as written for this system, **Change** (records the next combination, Escape cancels), **Clear**.

Row states (from the existing `Registration`, extended):

| State | Display | Text key |
|---|---|---|
| `on` | The keys. | none |
| `off` | "Not set" and Set. | `settings.desktop.shortcutNone` (exists) |
| `refused` | Warning band: "Another app already uses X. Pick another, or try Y" with a button per alternative. | `settings.desktop.shortcutRefused` (exists, change to take the combo), `settings.desktop.alternatives` (exists) |
| `unavailable` | The row is disabled with the reason: "This desktop does not allow global shortcuts. The shortcuts in Sidevoice still work." On Wayland-with-portal, the portal's own text replaces the keys and **Change** opens the compositor's dialog. | new `settings.shortcuts.unavailable`, `settings.shortcuts.portalManaged` |
| `needsPermission` (new) | Row shows the permission row below. | new |
| `soon` | A "Soon" badge and a disabled Set (mockup, for "Say one thing"). | new `settings.shortcuts.soon` |

Rows (the operator picks):

1. **Hold to talk** (`settings.shortcuts.global.holdToTalk`). Press opens, release mutes.
2. **Mute or unmute** (`settings.shortcuts.global.mute`, the existing row).
3. **Say one thing** (`settings.shortcuts.global.oneTurn`, "Soon").

Details and where research differs from the mockup:

- **Differs, the default for hold-to-talk.** The mockup shows `⌥Space` on macOS and `Alt+Space` on Windows/Linux as
  an example. Option-only combinations are the ones macOS 15.0 refused for sandboxed apps (restored in 15.2, mixed
  reports); Option+Space types a non-breaking space in text fields; `Alt+Space` is Windows's window menu. Spotlight-style
  launchers also commonly use these. **Recommendation: no default for hold-to-talk**, or a combination with Ctrl and Shift
  until real-machine tests say otherwise. Whatever the default, it is the operator's decision (open question 1).
- **Differs, the default for mute.** The mockup shows `⌘⇧D` / `Ctrl+Shift+D`; the app today uses `Ctrl+Alt+M`
  (desktop#10 still open). `⌘⇧D` is a common in-app shortcut in browsers (bookmark all tabs, **Inference**) and a global
  registration takes it from every other app while Sidevoice runs. Keep `Ctrl+Alt+M` and its alternatives unless the
  operator decides (open question 2).
- **The description of hold-to-talk.** The mockup says "let go to send". The in-app behaviour is "let go and the mic mutes
  again" (`settings.shortcuts.holdToTalkHint`). Reuse the existing text unless the page changes (open question 7).
- **Clash row.** The mockup's warning band with "Use ⌘⇧M" matches the existing `refused` plus `alternatives`. The
  alternatives are a fixed list (`SHORTCUT_ALTERNATIVES`) built for `Ctrl+Alt+M`; with three global actions the list
  must be per-action and not collide with each other (core `settings.rs`, a small change). A shortcut given to two
  actions at once is refused by the app before the system is asked: `settings.shortcuts.duplicate` ("X is already used
  for Y").
- **Windows AltGr.** A recorded `Ctrl+Alt+<letter>` shows the existing known-cost note on Windows layouts that use
  AltGr (the app cannot read the layout today; **not implemented**, so list it as a note, not a state).

### 6.2 Group "Permission, only if a shortcut needs it"

The mockup draws it macOS-only with Input Monitoring, "only for hold-to-talk on a single key like Fn or Right ⌥". The
research agrees: key combinations need nothing on macOS, Windows and X11. **Differs / additions:**

- The row appears **only** when a configured shortcut needs a permission: macOS single-key trigger (Input Monitoring).
  It is not shown for a person who only uses combinations.
- **Wayland needs a row too**, which the mockup does not have: the portal's consent. Show it as
  "Allow in your desktop's shortcut dialog" with a Set-up button that opens `BindShortcuts`, and the `unavailable` state
  above when the compositor has no portal.
- States: `notNeeded` (hidden), `notAsked` (Allow button), `granted` (hidden or a quiet "Allowed"), `denied`
  (explains where to switch it on, with the system page opened by the button; the shortcut stays off).
  Text keys: `settings.shortcuts.permission.inputMonitoring`, `.why`, `.allow`, `.denied`,
  `settings.shortcuts.permission.portal`.
- Windows and X11: no row ever.

### 6.3 Group "Headset buttons" (desktop and phone)

The mockup lists Press, Double press and "Outside a call". Aligned with research:

| Row | Options | Notes |
|---|---|---|
| **Press** (play/pause, AirPods squeeze, the call button) | Mute or unmute (today's behaviour, default), Say one thing (when A3 exists), Skip what is playing, Nothing | The platform decides what a press sends; the description names the buttons. |
| **Double press** | Skip what is playing, Mute or unmute, Nothing | **Differs**: only shown where the headset sends next-track. Today's code has no mapping (`docs/MEDIA_CONTROLS.md`); ship with the default Nothing, and let the operator choose whether Skip is the default (question 5). |
| **Outside a call** | Leave to other apps (default), Join the call | **Differs, recommend dropping "Join the call" for now.** It needs a media session or Now Playing claim with no call, which the repo avoided (empty Lock Screen card on iOS; the Android session lives only with the call). |

Not in the mockup, but real: **Hold to talk from a headset is not offered**, because press-and-release is not guaranteed
(section 5). On iOS in CallKit mode the system, not the app, decides what a press does, so the **Press** row shows a note
"This follows your phone's call controls" and its choices are disabled there. On macOS, Windows and Linux the row is
shown where a path exists today: macOS (MediaPlayer) yes; Windows and Linux only once desktop#196 lands, until then the
rows are hidden, not shown dead.

### 6.4 Group "In Sidevoice" (fixed, read only)

Matches the mockup and the existing `PageShortcuts`, read from `SHORTCUTS` in `state/shortcuts.ts`: Hold to talk
(Space), Mute or unmute (Cmd/Ctrl+D), Skip (Cmd/Ctrl+S). The mockup shows no Settings control here, which agrees with
today ("fixed, there is nothing to set"). Shown wherever a keyboard exists; hidden on a phone. Differences between the
two groups to say once: global hold-to-talk needs a combination with a modifier; in-app Space does not.

## 7. Bridge API proposal

Principle: **extend `MuteShortcutBridge`, do not duplicate it**, and keep the one seam for "a global shortcut that does an
action". Today `host.muteShortcut` is single-purpose (`get`, `set(accelerator)`, `subscribe`).

Proposal: a keyed generalisation, with `muteShortcut` kept as an alias so the pinned web and the desktop do not need to
change together (the repo's bridge tests assert the exact `host` keys, `test/bridge.test.mjs:1385-1389`).

```ts
type ShortcutAction = "mute" | "holdToTalk" | "oneTurn";   // oneTurn: later

interface ShortcutsBridge {                                  // host.shortcuts (new)
  get(): Promise<ShortcutsStatus>;
  set(action: ShortcutAction, accelerator: string | null): Promise<GlobalShortcut>;
  subscribe?(listener: (status: ShortcutsStatus) => void): () => void;
  /** Ask a permission the status named; never called by the app by itself. */
  request?(permission: "inputMonitoring" | "portal"): Promise<ShortcutsStatus>;
}
interface ShortcutsStatus {
  shortcuts: Record<ShortcutAction, GlobalShortcut>;
  permission: { kind: "inputMonitoring" | "portal"; state: "granted" | "notAsked" | "denied" } | null; // null: none needed
  headset: HeadsetStatus | null;                               // null: no headset path on this host
}
// GlobalShortcut = today's MuteShortcut, with these additions:
//   state gains "needsPermission"; "unavailable" stays;
//   managedBySystem: boolean   // Wayland portal: the label is the portal's text, not recordable
//   conflictWith?: ShortcutAction  // two actions on one combination
interface HeadsetStatus {
  press: "mute" | "oneTurn" | "skip" | "none";
  doublePress: "skip" | "mute" | "none" | null;   // null: this headset path cannot send it
  outsideCall: "leave" | "join";
}
// host.shortcuts.setHeadset(partial: Partial<…>) is the one writer; the app keeps the choice with its settings.
```

- `host.muteShortcut` remains as a thin view of `shortcuts.mute`, so the web that is pinned today keeps working. Add
  `"unavailable"` to the web's `MuteShortcut.state` type (the drift in section 1.2) in the same web PR.
- **Press and release to the page.** `Command` gains `HoldStart` and `HoldEnd` (`desktop/core/src/bridge.rs`), run as
  `__sidevoiceHost.run({command:"hold-start"})` / `"hold-end"`, mapped in `shared/tauri/js/members.js` to new
  `sidevoiceActions.holdStart()` / `holdEnd()` that set and clear `holding` **without** the focus/visibility release the
  keyboard path has. `OneTurn` is a third command and needs a page hook on the turn's end; **Inference**: the page has
  `cancelInput` and the `holding` state but this document did not find an existing "turn sent" signal; it needs web work.
- **Handler change:** `shortcut_access.rs` handler must read `event.state()` for `Released`, and carry which action a
  registered shortcut belongs to (the plugin hands the `Shortcut`, not an id). Today `replace()` calls `unregister_all()`
  and registers one; it becomes a diff over three.
- **Safety net for stuck hold.** A missed `Released` (the Windows poll thread, a lost X11 grab, the app losing focus
  mid-hold) would leave the mic open. Add a host-side timeout (the longest sensible turn, an operator choice, question 8)
  and release on call end, on `HoldEnd` never seen after screen lock or sleep. **Inference**; needs testing.
- **Mobile:** `docs/MOBILE.md` states the phone never offers `muteShortcut`; keep that. `host.shortcuts` returns
  `shortcuts` empty and `headset` set there. The mobile Android `CallService` and iOS `MediaControls` read the same
  `HeadsetStatus` for the press and double-press mapping by extending `media_command_action` (`headset.rs`) rather than
  adding a second table.
- **Docs to update with the implementation:** `docs/BRIDGE.md`, `docs/SHORTCUTS.md` (rename or split: it is the
  mute shortcut's file), `docs/MEDIA_CONTROLS.md`, `docs/MOBILE.md`.
- **Public information:** the implementation PRs touch user-visible behaviour (a setting, a permission, platform support),
  so each needs a landing change or a linked `sidevoice/landing` issue (`AGENTS.md`). This design PR does not.

## 8. Open questions for the operator

Each has a recommended option.

1. **Default for global hold-to-talk.** (a) None, the person records one. (b) `Ctrl+Shift+Space`-style combination, or (c) the
   mockup's `⌥Space` / `Alt+Space`. **Recommend (a)**, then (b) after device tests. (c) hits the macOS Option restrictions,
   the non-breaking-space character, and Windows's window menu.
2. **Default for global mute.** (a) Keep `Ctrl+Alt+M` (shown ⌃⌥M), (b) the mockup's `⌘⇧D` / `Ctrl+Shift+D`, (c) none.
   This is the open desktop#10. **Recommend (a)** until tested on all platforms, with the known AltGr cost on Windows
   documented. If (b), it clashes with common in-app uses of the same keys.
3. **Single-key triggers (Fn, a lone modifier) for hold-to-talk.** (a) Not in the first version, no permission at all on any
   OS except Wayland's portal, (b) macOS only, behind the Input Monitoring row, (c) all OSes with a low-level hook on
   Windows. **Recommend (a).** It matches "ask only when needed" with zero asks on macOS, Windows and X11.
4. **Wayland.** (a) Portal support in the first version, (b) show `unavailable` and point to the in-app keys until later.
   **Recommend (b) first, (a) as phase 3**, since a portal client is new code and compositor support is uneven (GNOME unconfirmed).
5. **Headset presses.** Press: **Mute or unmute** (keep). Double press: (a) skip what is playing, (b) nothing.
   **Recommend (b) as default** with (a) selectable: skip is a destructive action on a reply the person may want.
6. **Headset outside a call.** (a) Leave to other apps (b) Join the call. **Recommend (a)**: (b) has the empty-card cost.
7. **What "release" means for hold-to-talk.** (a) Mute only, as in-app Space (the turn is closed by the existing detection),
   (b) mute and force the turn to send now. **Recommend (a)** for parity; (b) is a page change with a "send now" action.
8. **A stuck-hold timeout.** (a) None, (b) 60 s, (c) the call's longest turn. **Recommend (b)**, as the safety net.
9. **Headset hold-to-talk.** Ruled out for now (section 5). Confirm, or ask for a device test first.
10. **Wire it through the web release or the page host first?** Un-hide the Shortcuts section (web#148) in the same
    release as the first desktop change, or ship the desktop side behind the existing `muteShortcut` alias first.
    **Recommend the alias first**, so the pinned web keeps working.

## 9. Phased plan

1. **Phase 0, verify what exists** (small, no feature): a device test of the current `Ctrl+Alt+M` on macOS, Windows and
   Linux X11, and of keyboard media keys with the call on (`docs/MEDIA_CONTROLS.md` last paragraph). Record versions.
   Fix the web's `MuteShortcut.state` type drift. Decide questions 1-3.
2. **Phase 1, global hold-to-talk and mute on macOS, Windows and X11** with combinations only: `Released` handled in
   `shortcut_access.rs`, the action carried per shortcut, `HoldStart`/`HoldEnd` commands and the page's focus-free hold,
   the stuck-hold timeout, tests with a fake plugin. Settings: un-hide Shortcuts, the "Anywhere" group, the clash band,
   the `unavailable` state. No permission row. Update `docs/BRIDGE.md` and `docs/SHORTCUTS.md`. Landing issue.
3. **Phase 2, headset.** Press and double press mapping on iOS (MediaPlayer path), Android (`MediaSession`) and macOS by
   extending `media_command_action`; the Headset group. Test on real headsets: wired, Bluetooth, AirPods. Windows SMTC and
   Linux MPRIS follow desktop#196.
4. **Phase 3, platform extras.** Wayland portal client; macOS Input Monitoring for single-key triggers if question 3 says
   (b); a research pass on the portal crate.
5. **Phase 4, "Say one thing".** Needs the page to expose a turn-sent signal and an auto-mute; then the Set button in the
   mockup's row, and the headset Press option.

## Sources

Primary, read on 2026-11-05:

- global-hotkey changelog and sources (macOS Carbon `kEventHotKeyReleased`, Windows release polling, X11 `KeyRelease`, media-key tap):
  [CHANGELOG](https://github.com/tauri-apps/global-hotkey/blob/dev/CHANGELOG.md),
  [macOS](https://github.com/tauri-apps/global-hotkey/blob/global-hotkey-v0.8.0/src/platform_impl/macos/mod.rs),
  [Windows](https://github.com/tauri-apps/global-hotkey/blob/dev/src/platform_impl/windows/mod.rs),
  [X11](https://github.com/tauri-apps/global-hotkey/blob/dev/src/platform_impl/x11/mod.rs),
  [docs.rs 0.8.0](https://docs.rs/global-hotkey/0.8.0/global_hotkey/) (platforms: Windows, macOS, "Linux (X11 Only)").
- Tauri plugin: [changelog](https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/global-shortcut/CHANGELOG.md),
  [docs](https://v2.tauri.app/plugin/global-shortcut/), `ShortcutState::{Pressed, Released}` in the plugin source.
- Windows: [RegisterHotKey](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerhotkey) (no key-up message, `MOD_NOREPEAT`, fails when already registered),
  [LowLevelKeyboardProc](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelkeyboardproc).
- XDG portal: [GlobalShortcuts](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html),
  [interface XML](https://raw.githubusercontent.com/flatpak/xdg-desktop-portal/main/data/org.freedesktop.portal.GlobalShortcuts.xml).
- Android: [MediaSession](https://developer.android.com/reference/android/media/session/MediaSession) (all sessions handle media
  buttons since API 26; `setMediaButtonBroadcastReceiver`), [KeyEvent](https://developer.android.com/reference/android/view/KeyEvent)
  (only the one-line descriptions of `KEYCODE_HEADSETHOOK`, `KEYCODE_MEDIA_PLAY_PAUSE`, `KEYCODE_CALL`, `KEYCODE_ENDCALL`).
- Apple: [Input Monitoring](https://support.apple.com/guide/mac-help/control-access-to-input-monitoring-on-mac-mchl4cedafb6/mac).
- Secondary, for the macOS 15 Option-hotkey change:
  [Apple Developer Forums](https://developer.apple.com/forums/thread/763878?page=2),
  [Eternal Storms blog](https://blog.eternalstorms.at/2024/09/23/keyboard-shortcuts-using-option-and-or-shift-modifiers-only-no-longer-allowed-on-macos-sequoia/).
- Secondary, for Wayland compositor support: a [2024 article](https://dec05eba.com/2024/03/29/wayland-global-hotkeys-shortcut-is-mostly-useless/),
  a [niri PR](https://github.com/YaLTeR/niri/pull/1165). Not authoritative.

**Not checked** (pages returned no body text, or were not searched): Apple's `RegisterEventHotKey`, `CGEventTapCreate`,
`CGPreflightListenEventAccess`, `MPRemoteCommandCenter` and `AVAudioApplication.setInputMuted` reference pages (JavaScript-rendered, the
fetch returned only titles); any claim in this document about those that is not from the repo is labelled **Inference**.
Not checked: GNOME's portal support from a GNOME primary source; secure input behaviour on macOS; games and UIPI on Windows;
whether Bluetooth headsets send separate down and up for the call button; and whether CallKit's AirPods squeeze behaviour on
hardware matches `docs/MEDIA_CONTROLS.md`. Nothing in this document was run on a real machine.
