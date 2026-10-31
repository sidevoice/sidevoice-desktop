# macOS: what the app needs to work on first open

## Install and first open

A release's and the nightly's .dmg are signed with the Developer ID and notarized (RELEASING.md): they open like any
downloaded app. CI builds are ad-hoc signed; for those, see `FIRST_OPEN.txt` (in this directory). Short version:

```sh
# after dragging Sidevoice.app to /Applications
xattr -dr com.apple.quarantine /Applications/Sidevoice.app
open /Applications/Sidevoice.app
```

Without Terminal: open it once, then System Settings → Privacy & Security → "Open Anyway".
(Right-click → Open no longer bypasses Gatekeeper on macOS 15+; it still does on 14 and earlier.)

## The microphone, layer by layer

A `getUserMedia` call in the room page has to pass four gates. Each one is handled:

| Gate | What it needs | Where |
|---|---|---|
| Secure context | The page is a secure context | The bundled interface on `tauri://localhost` is one (verified in CI) |
| WebKit's per-origin decision | A `WKUIDelegate` answer for the origin | `on_permission_request` in `open_room` → `core/src/media.rs`: microphone, camera and anything else **denied to every page**, the bundled interface included: the app runs the voice call. (wry's default, with no handler, grants everything to every page.) |
| App privacy (TCC) | `NSMicrophoneUsageDescription` in `Info.plist` — without it macOS kills the app on first capture | `desktop/packaging/macos/Info.plist`, merged by the bundler; CI checks it is in the built app |
| Hardened runtime | `com.apple.security.device.audio-input` entitlement | `desktop/packaging/macos/Entitlements.plist`; CI checks it is in the signature |

On macOS the app runs the voice call itself (cpal; docs/BRIDGE.md → "The voice call"), so the WebKit gate denies the
microphone to every page, the bundled interface included (the room window's permission handler, `src/lib.rs`), and the call's capture passes only the
other two. The page then cannot ask macOS for the microphone, nor read whether it may: the web's first-run setup asks
for it through `host.microphone` (`desktop/src/microphone.rs`; docs/BRIDGE.md → "The microphone permission"), which
reads the app's permission, shows macOS's prompt (`AVCaptureDevice.requestAccess`) and opens System Settings at the
microphone's pane after a denial.

macOS asks once ("Sidevoice quiere acceder al micrófono"). The answer is stored per app
signature: with ad-hoc signing each new build is a "new" app and may ask again; a release, signed with the Developer
ID, keeps its answer from one version to the next.
If it was denied: System Settings → Privacy & Security → Microphone, or
`tccutil reset Microphone ai.sidevoice.desktop`.

Echo cancellation is the web UI's own (`echoCancellation` constraints, `'all'` where supported);
WKWebView implements it with the system voice-processing unit.

## Sign-in

None. The app shows its own bundled interface and a device is admitted by pairing with the node (a one-time code
the node issues; docs/TARGETS.md). There is no oauth2-proxy or Google login in the app.

## Headset buttons (src/platform/media_controls/macos.rs)

What macOS offers a plain app, checked against Apple's documentation (2026-09-30):

| Mechanism | What it is | Availability | Used |
|---|---|---|---|
| `MPRemoteCommandCenter` + `MPNowPlayingInfoCenter` | Play/pause from **any** headset (Bluetooth AVRCP, wired, media keys) goes to the "Now Playing" app. On macOS the app must set `playbackState` whenever playback starts or stops, "otherwise remote control functionality may not work as expected". | macOS 10.12.2+ | **Yes, primary.** During a call the app is Now Playing; play → unmute, pause/toggle → toggle (the web interface's rule). |
| `AVAudioApplication.setInputMuteStateChangeHandler` | The system calls it "due to a Bluetooth audio accessory gesture (certain AirPods / Beats headphones)". macOS only. | macOS 14+ | **Yes, extra.** Installed during a call; the app keeps `isInputMuted` in step with its own mute. |
| CallKit | System calling UI for VoIP apps. | macOS 13+ (per Apple's metadata) | **No.** Neither mechanism above needs it on macOS. |
| Mic-in-use indicator | The orange dot in the menu bar. | always | Nothing to do: macOS shows it by itself whenever the app captures. |

Not verified on a Mac (no headset in CI), and what can go wrong:
- **HFP**: while the microphone is in use, a Bluetooth headset switches to the hands-free profile, and some headsets
  then send their button as an HFP command instead of AVRCP play/pause; macOS may not turn it into a media command.
  Wired headsets and the Mac's media keys are not affected.
- **The AirPods gesture** is documented for "the process doing the call's audio I/O". Here that is WebKit's GPU
  process (which captures for the page), not the app's own process, so the gesture may never reach the app.
- Another app that plays audio after the call started (a video) can become Now Playing and take the buttons.

To see what really arrives: the app log lists every button event the app receives (`headset …` lines: source and what
it did).
The page's own Media Session handlers are switched off in the app (`host.mediaKeys = "native"`), so one click never
toggles twice. CI simulates a call on a macOS runner and checks the app becomes Now Playing and lets go after.

## Background behaviour

- Closing the window hides it; the call continues. The app stays in the menu bar and the Dock.
- The room webview has background throttling **disabled** (`BackgroundThrottlingPolicy::Disabled`,
  macOS 14+), so a hidden window keeps capturing and playing. On macOS 13 WebKit may throttle
  timers of a hidden page; audio capture continues.
- During a call, with the window behind other apps (or hidden), the call controls card floats over them: a
  non-activating panel, so clicking it never takes the focus from the app the person is in, on every Space and over
  full-screen apps (docs/BRIDGE.md → "The call controls card"). The global mute shortcut is ⌃⌥M (⌘⇧M is VS Code's
  Problems panel); VoiceOver also listens to ⌃⌥ while it is on.
- Quit: tray → "Quit Sidevoice" or ⌘Q.

## What signing and notarization would need

1. An Apple Developer Program membership (99 USD/year) → a **Developer ID Application** certificate.
2. CI secrets: the certificate as base64 `.p12` + password (`APPLE_CERTIFICATE`,
   `APPLE_CERTIFICATE_PASSWORD`), `APPLE_SIGNING_IDENTITY`, and for notarization an App Store
   Connect API key (`APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_PATH`) or Apple ID +
   app-specific password + team ID. The Tauri bundler reads exactly these variables.
3. Change `bundle.macOS.signingIdentity` from `"-"` to the identity (or leave it to the env var),
   keep `hardenedRuntime: true` and the same entitlements.
4. Let the bundler build the `.dmg` too (it signs and notarizes it) and staple the ticket.
   Result: opens with a double click, no Gatekeeper dialog, and the microphone permission survives
   updates (stable signature).
5. Auto-update (`tauri-plugin-updater`) needs signed builds plus an update signing key: in place, RELEASING.md →
   Updates. An update replaces the .app with one signed by the same Developer ID, so the microphone permission and
   the keychain's trust carry over, from the nightly to a release too.

## iPhone (out of scope)

Tauri 2 builds iOS apps, but: an Apple Developer account and provisioning profiles; the WKWebView
code path is the same, but background audio needs the `audio` background mode and an
`AVAudioSession` category set natively; no tray or global shortcut, so the bridge's commands map
to lock-screen / Control Center controls (`MPRemoteCommandCenter`) instead; distribution via
TestFlight.

## Inspecting the webviews (nightly and dev builds)

A nightly and a dev build have hidden developer options in the room's Settings (click the version text 10 times in
5 seconds) with a "Web Inspector" switch; a release has neither the options nor an inspectable webview. The switch is
off by default and is kept (docs/BRIDGE.md → "The developer options"). With it on:

1. Safari → Settings → Advanced → "Show features for web developers".
2. Safari → Develop → *this Mac's name* → Sidevoice → the room (`tauri://localhost/voice/`) or the call controls card
   (listed while it exists).
3. To see what still paints on the CPU: Layers tab → "Show compositing borders" and paint flashing; Timeline →
   "Rendering Frames" or "Layout & Rendering" to see each paint.

Without Safari, the same section's "Compositing borders" and "Repaint counters" switches draw it over the app itself:
each GPU layer outlined, and on each layer the number of times the CPU repainted it.
