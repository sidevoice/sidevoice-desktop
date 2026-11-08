# Global shortcuts

The app registers up to three global shortcuts, set only in the web's Settings › Shortcuts (`host.shortcuts`,
docs/BRIDGE.md → "The shortcuts"): **mute / unmute**, **hold to talk** and **say one thing**. Design: sidevoice/sidevoice-desktop#447.
Everything below the first two sections was checked on 2026-10-01 and 2026-11-05 against the sources listed at the end;
**nothing here was tried on a real machine** (the device checklist is in the pull request that added it).

## The actions

| Action | Does | Through the page |
|---|---|---|
| Mute / unmute | The tray's and the card's mute: toggles the microphone | `toggle-mute` → `toggleMic()` |
| Hold to talk | While it is held and the microphone is muted, the microphone is open; let go, it is muted again. It does nothing if the microphone was already open | `hold-start` / `hold-end` → `holdStart()` / `holdEnd()`: the Space hold, kept down by the app and **not** let go of when the page loses focus |
| Say one thing | The microphone opens for one turn and mutes again when it is sent or taken back; a second press ends it | `one-turn` → `oneTurn()` |

A default is a state, not a value (operator, 2026-11-06; docs/BRIDGE.md → "Defaults are a state"): the settings keep a shortcut absent while the
person never chose, and the keys as chosen otherwise, even when they are the default's; `restore` makes it absent again. A default that changes later moves only
those who never chose. Once, a stored value equal to this system's default became absent (`shortcutsDefaultsMigrated`).

Defaults (decided by the operator, 2026-11-05): mute is **⌃⌥M on macOS** and **Ctrl+Alt+Shift+M on Windows and Linux**
(`DEFAULT_MUTE_*`, core `shortcuts.rs`, chosen per system by `platform::shortcut_access::default_mute`); hold to talk and
say one thing have **none**. The old default on Windows and Linux, Ctrl+Alt+M, is AltGr+M on many layouts, and
Ctrl+Shift+M is Teams' and Discord's. A person still on the old default is moved to the new one, **once** (settings
`shortcutsMigrated`); a shortcut chosen since stays whatever it is, and so does one chosen again after the move. (The Shortcuts
section was hidden in the page until now, so nobody could have chosen Ctrl+Alt+M on purpose: the move cannot undo a choice.)

## How a shortcut is written

A key is named by the physical key, never by the character it types (core `shortcuts::key_name`, from macOS's virtual key code): `NumpadAdd`,
`NumpadSubtract`, `NumpadMultiply`, `NumpadDivide` for the keypad (read `Num +`, `Num -`, `Num *`, `Num /`) and `Minus`, `Slash`,
`BracketRight` and the like for the main row, as `global-hotkey` registers them (docs/BRIDGE.md lists them). Older accelerators written
with the character still read as the same keys.

Keys together (`Ctrl+Alt+Shift+M`, `Cmd+Shift+M`, `F9`) or a single key with how it is pressed (`Fn:hold`,
`RightControl:double`, `F13:tap`), the same strings the page writes (docs/BRIDGE.md has the grammar). Press and release:

- A combination of mute or say one thing acts when it goes down. A combination of hold to talk is held: the hot key's `Pressed`
  starts it and its `Released` ends it (`tauri-plugin-global-shortcut` 2.4 / `global-hotkey` 0.8: Carbon's pressed and released events on
  macOS; on Windows `WM_HOTKEY` then a 50 ms poll of `GetAsyncKeyState`, so the release can be up to 50 ms late; X11 `KeyRelease`).
- A single key is a gesture (core `shortcuts::KeyMachine`, pure logic with the time passed in, tested): `hold` is down to up; `tap` is down and
  up in under 400 ms with no other key between, so ⌘C on the right Command is not a tap; `double` is two such taps, the second press within
  400 ms of the first one's release.
- **One key, several ways.** The same physical single key may carry hold, tap and double, one action each (`RightShift:hold` for hold to
  talk and `RightShift:double` for say one thing). Only then are they told apart by time (a key with one way acts as above):
  hold starts after 200 ms down (the microphone opens then, the key still down); a tap is a press under 200 ms with no second press within
  400 ms, and acts when those 400 ms are up if the key also has a double; a double is a second press within 400 ms of the first one's
  release, under 200 ms (400 ms with no hold on the key), and acts when it comes up, so the microphone never flashes for it. A second press
  that goes on past 200 ms is a hold. Another key pressed meanwhile makes the key a modifier of it, and nothing acts. That includes another bound single key pressed (Fn held, then Right Shift: Fn never opens the microphone) and, where the listener runs (a modifier or Fn is bound; macOS), any key typed; with only F-keys bound nothing listens to the keyboard, so typing during a pending F-key tap is not seen and the tap still acts. A machine woken from a sleep or a locked screen forgets its press instead of acting late. Two actions on the same
  key in the same way are the only duplicate (the first of mute, hold to talk and say one thing keeps it; the other is `refused`, and the
  answer names who holds it). An F-key has one hot key between its ways. Gaps: a pending tap is dropped, not fired, when its second press
  turns out to be a long one; and a double that hold-to-talk shares a key with cannot be told from two taps of a hold shorter than 200 ms.
- Which keys each action takes: hold to talk a combination or `:hold`; mute and say one thing a combination, `:tap` or `:double`.

## Per system

| | macOS 14 / 15 | Windows 11 | Linux X11 | Linux Wayland |
|---|---|---|---|---|
| Combinations | Carbon `RegisterEventHotKey`: no permission | `RegisterHotKey`: no permission | `XGrabKey`: no permission | `unavailable` (no backend in `global-hotkey`) |
| F-keys on their own (`F9`, `F13:hold`) | the same hot key: no permission | the same | the same | `unavailable` |
| Fn and Right Command / Alt / Control / Shift | a **listen-only `CGEventTap`** on `flagsChanged` and `keyDown`, made only while one is set and **Input Monitoring** is granted | `unavailable` | `unavailable` | `unavailable` |
| Recording a key in the page (`record`) | the app's own local monitor of keys in its windows (Fn included, **no permission**) | the page records | the page records | the page records |
| Hold to talk on an F-key | recorded alone it is `F10:hold`, a single key held as a right-hand modifier is; its hot key reports the release (Carbon `kEventHotKeyReleased`). F1 to F20 only: a Mac has no F21 to F24 (`global-hotkey` maps none, and the recorder hears none) | the release is looked for every 50 ms after `WM_HOTKEY` (`global-hotkey`): it arrives up to 50 ms late, and a release and the next press within one look report none, so the second press ends the first there (`KeyMachine`), and a quick double stays a double | `KeyRelease`, with detectable auto-repeat | `unavailable` |
| Headset's press | `MPRemoteCommandCenter` (the system's play/pause; a double press is `nextTrackCommand`) and the AirPods gesture: what the person chose | no path (`headset: null`) | no path | no path |
| Screen locked → a hold is let go | yes (`CGSessionCopyCurrentDictionary`) | yes (the input desktop cannot be opened) | no observation | no observation |

On the phones (iOS, Android) Settings › Shortcuts shows the headset's two rows and nothing else: the phone reports `shortcuts: {}` and `inApp: false` (docs/BRIDGE.md → "The mobile app"; docs/MEDIA_CONTROLS.md).

Single modifier keys on Windows and Linux are `unavailable` on purpose: `RegisterHotKey` and an X11 grab take a key with its
modifiers, and the alternatives (a `WH_KEYBOARD_LL` hook, raw input, `/dev/input`) hear every key; a keyboard hook is what keyloggers use and
what antivirus heuristics look at, so the app does not install one for a convenience. F-keys (F13 to F24 are the usual lone keys) work on every
system that has hot keys and need nothing.

### Input Monitoring (macOS, single keys only)

- Nothing is asked while only combinations and F-keys are set. **No tap is created** and the permission is not read for any purpose
  but the status.
- Setting Fn or a right-hand modifier makes `permission` `{kind: "inputMonitoring", state}` and the shortcut `needsPermission` until the permission
  is there (`CGPreflightListenEventAccess`, which asks nothing). The page's one amber warning has an Allow button, which is
  `request()`: **only then** `CGRequestListenEventAccess` shows the system's prompt (once; after a refusal the system shows none, so the app opens
  Privacy & Security › Input Monitoring). `denied` is "asked here, and still not granted".
- The app then looks once a second for a minute, and on the next `get`, so the listener starts as soon as the permission is on, with no restart.
- The tap is a listen-only one, so it can drop nothing and needs no Accessibility. The system may turn a listen-only tap off; the app turns it back on.
- Fn: the Globe key does something else by default (emoji picker, dictation). The page tells the person to set it to "Do nothing" in Keyboard settings.

## The stuck hold

A hold whose `Released` never comes would leave the microphone open. The app lets it go (the page is told `hold-end`) when:

1. 60 seconds pass without a release (`STUCK_HOLD`);
2. the machine sleeps (a gap between two looks, one a second, of more than 5 s on the wall clock);
3. the screen is locked (macOS, Windows; Linux has no observation the app reads);
4. the call ends, or the shortcut is changed or cleared while held.

A release that arrives after the app let go is nothing (the machine forgets the press). Core `shortcuts::Watch` and `Hold` are the rules; `src/shortcuts.rs`
looks every second while a hold lasts and never otherwise.

## "Another app already uses it" on Windows

Reported: the mute shortcut stays refused on Windows. **Not reproduced here** (this is a Linux machine); the code shows how it can happen, and
the cause is fixed and covered:

- `replace()` called the plugin's `unregister_all()`, which empties the plugin's own table **before** the system is asked, and stops at the first
  error. A `UnregisterHotKey` that failed left `RegisterHotKey`'s registration alive in the system while the app's books said it held none. Every
  later `register` of the same keys then answered "already registered" (`ERROR_HOTKEY_ALREADY_REGISTERED`), which the app reported as
  **another app's**: refused for good, until a restart.
- Every refusal was reported the same, whether the system said the keys were taken or the key could not be registered at all.
- Ctrl+Alt+M on a layout with AltGr is not a refusal (the registration works and then eats AltGr+M), which is why the Windows default changed
  and why the page warns on Ctrl+Alt+letter (below).

Now `replace()` is a diff (core `shortcuts::reconcile`, one entry per action): only what changed is let go of or registered, a key already held is
not asked again, and a key the system calls taken is first let go of by the app (its own stale registration goes, another app's cannot be touched) and
asked once more before it is believed to be another app's. Tests (core, with a fake system) cover: changing one shortcut leaves the others
registered, an unchanged one asks the system nothing, a stale registration of the app's own does not keep it refused, another app's key is refused with
alternatives and nothing left registered, and two actions on the same keys.

## What the app offers instead, and clashes

When the system refuses a shortcut the answer is `refused` with the action's own suggestion from `ALTERNATIVES`, if the system takes it and nothing
else of ours asks for it, for the page to offer in one click: **Ctrl+Shift+F9** for mute, **Ctrl+Shift+F10** for say one thing and **Ctrl+Shift+F11**
for hold to talk (⌃⇧F9 to F11 on a Mac), on every system. They are keys every keyboard has (no F13 to F19), and were checked against the systems' lists:

- macOS: Option+Shift with the volume and brightness keys makes fine adjustments (F1, F2 and F10 to F12 when the F-keys act as media keys), so no
  Option+Shift here; Control+F2 to F8 is keyboard navigation, so none of those. Control+Shift with F9, F10 and F11 appears nowhere in
  [Mac keyboard shortcuts](https://support.apple.com/en-us/102650) (read 2026-11-06; its only F-key entry is Fn-F11, Show the desktop).
- Windows: F10 alone opens the menu bar and Shift+F10 is the context menu, but Ctrl+Shift+F10 is in no list; Ctrl+Shift with F9, F10 and F11 appears nowhere in
  [Keyboard shortcuts in Windows](https://support.microsoft.com/en-us/windows/keyboard-shortcuts-in-windows-dcc61a57-8ff0-cffe-9796-cb9706c75eec)
  (read 2026-11-06; plain F11 maximizes a window).
- Linux: the GNOME and KDE lists in Sources were **not** re-read for these three; Ctrl+Alt+F1 to F12 (virtual terminals) is the one family
  known to be taken, and none of these has Alt.

Not tried on a machine; a keyboard whose F-keys are media keys by default needs Fn for them, as for any F-key shortcut. The page itself also warns on the same keys set for two actions and on
Ctrl+Alt+letter on Windows and Linux. On macOS the app also says when VoiceOver is on and the shortcut holds Control and Option together.

## Decisions (desktop#447, section 8) as they were decided

Operator, 2026-11-05. The questions are those of the design document.

| # | Question | Decided |
|---|---|---|
| 1 | Default for global hold to talk | **None**: the person records one |
| 2 | Default for global mute | ⌃⌥M on macOS; **Ctrl+Alt+Shift+M on Windows and Linux** (this closes desktop#10) |
| 3 | Single keys (Fn, a lone modifier) | **Yes**, on macOS, through the listen-only tap and Input Monitoring asked only when one is chosen; F-keys everywhere; no permission asked otherwise |
| 4 | Wayland | `unavailable` for now (portal client later) |
| 5 | Headset presses | Single press: mute (as now), selectable; double press: the system's next track (macOS), `none` until chosen |
| 6 | Headset outside a call | Leave the buttons to other apps |
| 7 | What release means for hold to talk | Mute only, as the in-app Space |
| 8 | Stuck-hold timeout | 60 s, and on call end, screen lock and sleep |
| 9 | Headset hold to talk | Ruled out |
| 10 | Web release or page host first | The alias first: `host.muteShortcut` stays |

## Checked against the systems' published default shortcuts

Checked on 2026-10-01 against the lists and sources below. Nothing here was tried on a real machine.

| | `Ctrl+Alt+M` / `⌃⌥M` | `Ctrl+Alt+Shift+M` / `⌃⌥⇧M` | `Ctrl+Alt+Shift+K` / `⌃⌥⇧K` |
|---|---|---|---|
| macOS default shortcuts | not used | not used | not used |
| macOS VoiceOver (off by default) | VO-M: move to the menu bar | VO-Shift-M: open a shortcut menu | VO-Shift-K: Option keys control VoiceOver, on or off |
| Windows 11 default shortcuts | not used | not used | not used |
| Windows Magnifier (off by default) | cycle through views | not used | not used (`Ctrl+Alt+K` without Shift reads the next sentence) |
| GNOME (upstream and Ubuntu) | not used | not used | not used |
| KDE Plasma (global) | not used (Konsole's own "Previous Profile" while Konsole has focus) | not used | not used |

None of the three is a default shortcut on any of the four systems. Two assistive tools that are off by default use
them. That is why the app warns while VoiceOver is on. No warning exists for Magnifier yet.

### Windows: Ctrl+Alt is also AltGr

Windows treats Ctrl+Alt as AltGr on many keyboard layouts, so a global `Ctrl+Alt+<letter>` can stop the person typing
that layout's AltGr character. Microsoft's guidance is not to use Ctrl+Alt combinations.

| Shortcut | Character it can block | Layouts |
|---|---|---|
| `Ctrl+Alt+M` | µ | German, Swedish, Norwegian, Danish, Finnish, Dutch, Icelandic, Greek Latin, US-International |
| `Ctrl+Alt+M` | § | Polish (214), Romanian (Legacy), Serbian (Latin), Slovenian |
| `Ctrl+Alt+Shift+M` | W | Latvian |
| `Ctrl+Alt+Shift+K` | Ķ | Latvian |

This is not a system shortcut collision, so #4's check passes, but it is the cost of the old default on Windows and Linux, and why the
default there is now `Ctrl+Alt+Shift+M` (only Latvian's W is left, and the page warns on any Ctrl+Alt+letter on those systems). macOS has no AltGr.

## Sources

- macOS:
  - [Mac keyboard shortcuts](https://support.apple.com/en-us/102650) (published 2026-09-14).
  - The VoiceOver command pages: [general](https://support.apple.com/guide/voiceover/general-commands-cpvokys01/mac),
    [orientation](https://support.apple.com/guide/voiceover/orientation-commands-cpvokys03/mac),
    [navigation](https://support.apple.com/guide/voiceover/navigation-commands-cpvokys04/mac),
    [text](https://support.apple.com/guide/voiceover/text-commands-cpvokys06/mac),
    [interaction](https://support.apple.com/guide/voiceover/interaction-commands-cpvokys07/mac),
    [search](https://support.apple.com/guide/voiceover/search-commands-cpvokys08/mac).
    "VO" is Control+Option (or Caps Lock).
- Windows:
  - [Keyboard shortcuts in Windows](https://support.microsoft.com/en-us/windows/keyboard-shortcuts-in-windows-dcc61a57-8ff0-cffe-9796-cb9706c75eec).
  - [Windows keyboard shortcuts for accessibility](https://support.microsoft.com/en-us/windows/windows-keyboard-shortcuts-for-accessibility-021bcb62-45c8-e4ef-1e4f-41b8c1fc87fd).
  - [Magnifier reading](https://support.microsoft.com/en-us/accessibility/windows/magnifier/how-to-use-magnifier-reading).
  - [Narrator commands](https://support.microsoft.com/en-us/windows/appendix-b-narrator-keyboard-commands-and-touch-gestures-8bdab3f4-b3e9-4554-7f28-8b15bd37410a).
  - [Win32 keyboard guidelines](https://learn.microsoft.com/en-us/windows/win32/uxguide/inter-keyboard) (on Ctrl+Alt).
  - AltGr characters per layout come from [kbdlayout.info](https://kbdlayout.info), a third-party site generated from
    the Windows layout files.
- GNOME:
  - [Useful keyboard shortcuts](https://help.gnome.org/users/gnome-help/stable/shell-keyboard-shortcuts.html).
  - The defaults in the settings schemas on each project's main branch, 2026-10-01:
    - gsettings-desktop-schemas `org.gnome.desktop.wm.keybindings` (`2b991c09`);
    - gnome-settings-daemon `org.gnome.settings-daemon.plugins.media-keys` (`bf7df434`);
    - gnome-shell `org.gnome.shell.keybindings` (`fae3b090`);
    - mutter `org.gnome.mutter` and `org.gnome.mutter.wayland` (`e0666ab5`).
  - Ubuntu's additions (`Ctrl+Alt+T`, `Ctrl+Alt+D`):
    [gnome-settings-daemon packaging](https://git.launchpad.net/ubuntu/+source/gnome-settings-daemon) and the
    ubuntu-settings gsettings override.
- KDE:
  - [Common keyboard shortcuts](https://docs.kde.org/stable_kf6/en/khelpcenter/fundamentals/kbd.html). It is older
    than the code, so the defaults were also read in the source on each project's master branch, 2026-10-01:
    - kwin `src/useractions.cpp`;
    - plasma-workspace `startkde/session-shortcuts/main.cpp`;
    - plasma-pa `src/kded/audioshortcutsservice.cpp`;
    - konsole `desktop/org.kde.konsole.desktop` and `src/session/SessionController.cpp`;
    - spectacle, plasma-desktop and kscreenlocker.

Not checked: Orca (GNOME's screen reader), the X11 `Ctrl+Alt+Backspace` option, Ubuntu Dock's own schema, and
distributions' own KDE defaults.

## Desktop platform boundary

`platform::application` (`configure`, `prepare`, `activate`, `handle_event`, `report`) owns single-instance setup, native launch policy, activation, reopen and diagnostic
observations. The common shell still shows/restores/focuses the room and keeps it alive when closed. macOS retains
its Regular activation policy, visible Dock tile, macOS 14+ activation and Dock reopen handling. Windows/Linux use
Tauri window focus and the existing single-instance plugin. No native accessibility equivalent is inferred there.

`platform::shortcut_access` owns the maintained global-shortcut plugin (which hot key is which action's, and the diff that registers them),
this system's default mute shortcut, live VoiceOver reservation and label style. `platform::key_capture` owns the keyboard listener
for single keys (macOS: the listen-only `CGEventTap`, Input Monitoring) and the capture of the next key for `record`
(macOS: a local `NSEvent` monitor); `platform::session` owns whether the screen is locked. Windows and Linux keep the honest defaults
(no listener, no capture; Windows can tell a locked screen, Linux cannot). The gestures, the registrations' diff, the safety net, the recorder's
rules and the status are core `shortcuts.rs`, and the orchestration is `desktop/tauri/src/shortcuts.rs`: none of them touches the system.
A backend failure leaves the app running and reports `unavailable` through `host.shortcuts` and `host.muteShortcut`; an
individual registration conflict is `refused`. An empty shortcut is `off`.

The plugin's [global-hotkey backend](https://docs.rs/global-hotkey/0.8.0/global_hotkey/) supports Windows, macOS and
Linux X11. Native Wayland reports `unavailable`, including sessions that also expose an XWayland `DISPLAY`; GTK's
actual display backend is checked. X11 registration remains subject to the window manager's restrictions.
Windows Magnifier/AltGr and Linux accessibility conflicts listed above remain unobserved, rather than reported safe.

Runtime verification is pending on macOS, Windows and Linux X11/Wayland. The portable tests check the rules (gestures, diff, migration,
safety net, recorder, status) and the unavailable and disabled states; cross-compilation checks the interface of the system-specific files; neither
establishes native focus, key delivery, the permission's prompt or assistive-technology behavior. Not known without a device: Fn (Globe) reaching the tap on
every keyboard and macOS version; `CGEventTap` and the Input Monitoring prompt on 14 and 15; the Windows report above; X11 key repeat during a hold (the server
may send a release and a press for each repeat, which would flap a held hot key: if so, the diff must debounce a release that a press follows at once).
