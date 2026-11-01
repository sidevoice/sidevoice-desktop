# Headset and media controls

The shared plugin's `app-tauri/src/platform/media_controls.rs` exposes `available`, `attach` and `update`. The desktop app
updates it from its `bridge_state`, the mobile app from its `call_state` (the page's report of the call); both run the
command on the room through `window.__sidevoiceHost.run`.
Where `available` is true the app offers `host.presence` (`{ media: "app" }`), independently of native voice. Startup attaches one
process-lifetime command sink; call reports and navigation resets update it.

| Host | Native controls | Page fallback during native voice | Physical buttons |
|---|---|---|---|
| macOS | Existing MediaPlayer Now Playing and AVAudioApplication mute gesture | Suppressed by `presence: { media: "app" }`, avoiding duplicate actions | Unverified on hardware |
| Windows | Unavailable (no `presence`) | Page Media Session and its Now Playing audio element | Unverified in WebView2/on hardware |
| Linux | Unavailable (no `presence`) | Page Media Session and its Now Playing audio element | Unverified in WebKitGTK/X11/Wayland/on hardware |
| iOS | MediaPlayer's Now Playing (lock screen, Control Center) and remote commands, and the AirPods mute gesture (iOS 17+): the macOS code (`apple.rs`) | Suppressed by `presence: { media: "app" }` | Unverified on hardware |
| Android | A `MediaSession` (app-tauri's Kotlin, `MediaControls.kt`), active during a call, shown by the call's foreground service's notification (MediaStyle) | Suppressed by `presence: { media: "app" }` | Unverified on hardware |

The page maps play to unmute, pause to toggle mute (including a pause delivered while muted),
and `togglemicrophone`, where the webview supports it, to the requested microphone state.
Hang-up removes the page handlers and pauses its Now Playing element. A dedicated hardware
hang-up gesture is not claimed. AirPods/Beats' native mute gesture is macOS's and iOS's (17 and later).

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
