# Headset and media controls

The shared plugin's `shared/tauri/src/platform/media_controls.rs` exposes `available`, `attach` and `update`. The desktop app
updates it from its `bridge_state`, the mobile app from its `call_state` (the page's report of the call); both run the
command on the room through `window.__sidevoiceHost.run`.
Where `available` is true the app offers `host.presence` (`{ media: "app" }`), independently of native voice. Startup attaches one
process-lifetime command sink; call reports and navigation resets update it.

| Host | Native controls | Page fallback during native voice | Physical buttons |
|---|---|---|---|
| macOS | Existing MediaPlayer Now Playing and AVAudioApplication mute gesture (opted in at launch, below) | Suppressed by `presence: { media: "app" }`, avoiding duplicate actions | Unverified on hardware |
| Windows | Unavailable (no `presence`) | Page Media Session and its Now Playing audio element | Unverified in WebView2/on hardware |
| Linux | Unavailable (no `presence`) | Page Media Session and its Now Playing audio element | Unverified in WebKitGTK/X11/Wayland/on hardware |
| iOS | No Now Playing and no remote commands, ever (`ios.rs`, desktop#464): during a call a Bluetooth headset is the call's hands-free (HFP), whose presses are call control that iOS gives to CallKit and never to `MPRemoteCommandCenter`, and Now Playing would only make the app look like a music source to a car (desktop#465). What reaches the app is the AirPods / Beats **Press to Mute and Unmute** (iOS 17 and later): an app without CallKit opts in by observing `AVAudioApplicationInputMuteStateChangeNotification` (Apple, WWDC23 "Enhance your app's audio experience with AirPods"), which the app does once, at launch, for the process's life (`apple::observe_input_mute`, as on macOS; the app log says `headset: press to mute opted in`), before any call's audio session is active. iOS then mutes the app's input itself, plays its tone and posts the notification, which brings the page's mute in step; the page's own mute goes the other way through `setInputMuted`, and the call's end gives the input back unmuted. iOS posts it only while a record session is active (the call's play-and-record, voice chat, desktop#466); Apple names no other condition, and voice-processing I/O is not one (`setInputMuted` mutes every input of the app). The symbols are looked up at run time, so iOS 15 and 16 launch. While the room's call is a CallKit call (docs/MOBILE.md → "The system's call") the headset's mute acts on it through CallKit, its mute reaching the page as the person's. Every command of the command center is let go (its targets removed and it disabled, no Now Playing info, `endReceivingRemoteControlEvents`) at launch, at a call's start and at its end; the app log says so (`call: now playing … released=…`). With Live Activities off, the lock screen shows nothing of the call | Suppressed by `presence: { media: "app" }` | Unverified on hardware |
| Android | A `MediaSession` owned by the call's foreground service (`CallService`), so it exists only during the call; the call's state and the commands go through shared/tauri's Kotlin (`MediaControls.kt`). No notification shows the session: the call's notification is the app's own card (docs/MOBILE.md → "The call surface", "Android's card"), so there is no media card; the session goes on taking a headset's or a car's buttons (mute and unmute), also while Telecom holds the call, which leaves a self-managed call's buttons to its app | Suppressed by `presence: { media: "app" }` | Unverified on hardware |

The page maps play to unmute, pause to toggle mute (including a pause delivered while muted),
and `togglemicrophone`, where the webview supports it, to the requested microphone state.
Hang-up removes the page handlers and pauses its Now Playing element. A dedicated hardware
hang-up gesture is not claimed. AirPods/Beats' native mute gesture is macOS's (14 and later, a handler block, `setInputMuteStateChangeHandler`,
macOS only) and iOS's (17 and later, a notification). The system gives the gesture only to a process that records and has opted in,
and on both the opt-in is observing `AVAudioApplicationInputMuteStateChangeNotification`: on macOS a handler without it is refused, the
system playing its rejection and showing "Cannot Control Mic with AirPods". So both apps opt in once, at launch, for the process's life
(`apple::observe_input_mute`, the rule `headset::gesture_step`; the app log says `headset: press to mute opted in`), before any call records;
outside a call the app does not record, so the gesture is not its own, and a press that still reaches it is answered "no call" (the
system's own refusal). During a call the system's input mute is kept in step with the call's microphone, and given back unmuted at its end.
What differs between the two is a private trait of `apple.rs` (`Os`, as every platform concern's), one method per difference,
implemented by `macos.rs` and `ios.rs`: the playback state (macOS sets it, iOS has none to set) and how the gesture is heard (macOS:
the handler, the notification observed only as the opt-in; iOS: the notification).

## What the single press does on the desktop

Where the app owns the headset's buttons (macOS, `available()`), the one command it hears, `MediaCommand::ToggleMute` (the system's
play/pause, **and** the AirPods mute gesture, which arrive the same way), does what the person chose in Settings › Shortcuts › Headset
buttons (`host.shortcuts.setHeadset({press})`, kept with the app's settings): **mute** (the default, as always), **say one thing**, **skip what
is playing**, or **nothing** (desktop `src/shortcuts.rs` `headset_press`). `HeadsetStatus` is `null` on
Windows and Linux, where the page owns the media session and the app has nothing to map. The double press (`doublePress`) is there on macOS: a
headset's or AirPods' double press of play/pause arrives through `MPRemoteCommandCenter` as `nextTrackCommand`, which `apple.rs` registers on
macOS (`Os::NEXT_TRACK`), under the same condition as the single press (while
the app shows the call as Now Playing; the handler is removed with the others), and sends as `MediaCommand::NextTrack`. It does what the person chose
(`host.shortcuts.setHeadset({doublePress})`, `headset_double_press`), **nothing** until chosen. A keyboard's next-track key arrives as the same command, so while
the app holds the session it triggers the double press's action too. Outside a call the app leaves the buttons to other apps
(`outsideCall`: `leave`), as the handlers are only attached while a call is on. A headset's hold to talk stays ruled out (desktop#447).
The phones answer the same two presses with the same choices (below, and docs/MOBILE.md → "The bridge", `shortcuts`).

On a phone the same rule holds for the native commands: play unmutes, pause toggles, and the single press does what the person chose
(`mobile/tauri/src/bridge.rs` (`headset`), kept in `headset.json`). On Android a headset's single press arrives as pause while the session
plays (the microphone is open) and as play while it is paused (muted). The double press is the next-track command too:

- **iOS**: only the single press reaches the app, and only as the system's mute (Press to Mute, AirPods and Beats; above): no
  double press, and no other action for the single press, reaches an app in a call without CallKit, and with CallKit the system
  holds the headset. So the phone reports what its presses can do (`media_controls::presses`, `HeadsetCapabilities::SYSTEM_MUTE`):
  `actions: {press: ["mute"]}` and `doublePress: null`, and the page shows the single press as a line, with no double press row
  (docs/BRIDGE.md → "The shortcuts"). A choice kept from an earlier build stays kept; here the single press mutes and the double press
  does nothing.
- **Android** has no remote-command center: the call service's `MediaSession` advertises skip-to-next, and gets `onSkipToNext`
  from the framework (PLAY_PAUSE or HEADSETHOOK pressed twice within the system's double-tap time), **only while the double press has
  an action**: advertising it makes the framework hold a single press for the double-tap time, so with none the single press is
  immediate. It is released with the service at hang-up.
- When the call in progress is the system's (the developer option was on when it started and the system took it; a call the system
  refused is the app's own, and the option changing mid-call applies to the next call), the system holds the headset: the single press mutes, the double press does nothing, and the
  rows say so read-only (`managedBySystem`).

Which of these a given headset sends is the headset's: AirPods can be set to other gestures in iOS settings, and some Bluetooth
headsets send a double press as something else.

`npm run web && npm test` runs the pinned page's Media Session and microphone fan-out bodies,
verbatim from its release bundle, against a live native-call bridge object with Tauri commands
recorded by a fake transport. It checks both desktop origins, mute/unmute delivery to the right
native call, Now Playing, handler teardown, and suppression with native media ownership.
It does not run a real audio backend or prove OS delivery. Its small extraction shim fails if a
future web pin changes the bundle layout; update the shim with that pin instead of copying the
page's implementation here. No vendored page code is changed.

Before declaring headset support verified, run a native call on each OS with real microphone
and output, test keyboard media keys and wired/Bluetooth play/pause while live and muted,
then hang up and confirm buttons stop controlling the call. On macOS also test the AirPods mute
gesture; on Linux repeat under X11 and Wayland. Record device/webview versions and which
unsupported gestures remain unavailable.

Native Windows SMTC and Linux MPRIS backends are tracked separately in
[desktop#196](https://github.com/sidevoice/sidevoice-desktop/issues/196). They do not block the
native voice pipeline. This extraction preserves the existing maintained objc2 bindings on
macOS and adds no new native backend or dependency.
