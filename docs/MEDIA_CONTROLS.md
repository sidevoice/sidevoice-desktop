# Headset and media controls

The shared plugin's `shared/tauri/src/platform/media_controls.rs` exposes `available`, `attach` and `update`. The desktop app
updates it from its `bridge_state`, the mobile app from its `call_state` (the page's report of the call); both run the
command on the room through `window.__sidevoiceHost.run`.
Where `available` is true the app offers `host.presence` (`{ media: "app" }`), independently of native voice. Startup attaches one
process-lifetime command sink; call reports and navigation resets update it.

| Host | Native controls | Page fallback during native voice | Physical buttons |
|---|---|---|---|
| macOS | Existing MediaPlayer Now Playing and AVAudioApplication mute gesture | Suppressed by `presence: { media: "app" }`, avoiding duplicate actions | Unverified on hardware |
| Windows | Unavailable (no `presence`) | Page Media Session and its Now Playing audio element | Unverified in WebView2/on hardware |
| Linux | Unavailable (no `presence`) | Page Media Session and its Now Playing audio element | Unverified in WebKitGTK/X11/Wayland/on hardware |
| iOS | While the room's call is a CallKit call (docs/MOBILE.md → "The system's call"), none: the lock screen shows the call, and a headset's buttons and the AirPods gesture act on it through CallKit (its mute reaches the page as the person's mute). Otherwise (CallKit refused the call, a try in Settings) MediaPlayer's Now Playing (lock screen, Control Center) and remote commands, the macOS code (`apple.rs`); the AirPods mute gesture on iOS 17+ through `AVAudioApplicationInputMuteStateChangeNotification` (iOS mutes the input itself; the app keeps the call in step), its symbols looked up at run time so iOS 15 and 16 launch. The remote commands' handlers are registered only while Now Playing shows the call. Now Playing gives way while the call's Live Activity runs **or is on its way** (from the call's start until it is requested, when Live Activities are on for the app): a handler registered even for the first report, before the main queue has requested the activity, makes the app the Now Playing app for the rest of the process, an empty card on the Lock Screen. With Live Activities off, Now Playing and the headset's buttons work from the call's first report. With them on, a call that starts while the app is in the background has no Now Playing and nothing on the Lock Screen until the app next comes to the front and requests the activity: deliberate, since Now Playing shown until then would leave the empty card once the activity came (the same holds after a failed request that a retry later makes good). Every command of the command center is let go (its targets removed and it disabled, no Now Playing info, `endReceivingRemoteControlEvents`) at launch, at a call's start, when Now Playing gives way, and at a call's end; the app log says what was enabled at the call's start and after, and whether the app's side let go (`call: now playing … released=…`; whether iOS really dropped the app as its Now Playing app only mediaremoted's log says) | Suppressed by `presence: { media: "app" }` | Unverified on hardware |
| Android | A `MediaSession` owned by the call's foreground service (`CallService`), so it exists only during the call; the call's state and the commands go through shared/tauri's Kotlin (`MediaControls.kt`). No notification shows the session: the call's notification is the app's own card (docs/MOBILE.md → "The call surface", "Android's card"), so there is no media card; the session goes on taking a headset's or a car's buttons (mute and unmute), also while Telecom holds the call, which leaves a self-managed call's buttons to its app | Suppressed by `presence: { media: "app" }` | Unverified on hardware |

The page maps play to unmute, pause to toggle mute (including a pause delivered while muted),
and `togglemicrophone`, where the webview supports it, to the requested microphone state.
Hang-up removes the page handlers and pauses its Now Playing element. A dedicated hardware
hang-up gesture is not claimed. AirPods/Beats' native mute gesture is macOS's (a handler block, `setInputMuteStateChangeHandler`, macOS only) and
iOS's (17 and later, a notification). What differs between the two is a private trait of `apple.rs` (`Os`, as every platform concern's), one
method per difference, implemented by `macos.rs` and `ios.rs`: the playback state (macOS sets it, iOS has none to
set) and how the gesture is heard.

## What the single press does on the desktop

Where the app owns the headset's buttons (macOS, `available()`), the one command it hears, `MediaCommand::ToggleMute` (the system's
play/pause, **and** the AirPods mute gesture, which arrive the same way), does what the person chose in Settings › Shortcuts › Headset
buttons (`host.shortcuts.setHeadset({press})`, kept with the app's settings): **mute** (the default, as always), **say one thing**, **skip what
is playing**, or **nothing** (desktop `src/shortcuts.rs` `headset_press`; the shared plugin is not changed). `HeadsetStatus` is `null` on
Windows and Linux, where the page owns the media session and the app has nothing to map. The double press (`doublePress`) is `null`: the
path the app listens on sends no next-track command (see below), so it cannot send one. Outside a call the app leaves the buttons to other apps
(`outsideCall`: `leave`), as the handlers are only attached while a call is on. A headset's hold to talk stays ruled out (desktop#447).
Mobile is out of scope here: it is owned by the call's surface (Live Activity, Android's session).

On a phone the same rule holds for the native commands: play unmutes, pause toggles. On Android a headset's single press
arrives as pause while the session plays (the microphone is open) and as play while it is paused (muted). There is no
next-track mapping (skip the reply) on any host: the desktop has none, and adding one is its own decision.

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
