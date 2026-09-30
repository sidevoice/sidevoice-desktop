# macOS: what the app needs to work on first open

## Install and first open (unsigned build)

See `FIRST_OPEN.txt` (it ships next to the `.dmg` in the CI artifact). Short version:

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
| Secure context | The page is `https://` (or `http://localhost`) | Settings refuse any other URL (`core/src/settings.rs`) |
| WebKit's per-origin decision | A `WKUIDelegate` answer for the origin | `on_permission_request` in `open_room` → `core/src/media.rs`: microphone **granted for the configured room origin only**, camera and every other origin denied. (wry's default, with no handler, grants everything to every page.) |
| App privacy (TCC) | `NSMicrophoneUsageDescription` in `Info.plist` — without it macOS kills the app on first capture | `src-tauri/Info.plist`, merged by the bundler; CI checks it is in the built app |
| Hardened runtime | `com.apple.security.device.audio-input` entitlement | `src-tauri/Entitlements.plist`; CI checks it is in the signature |

macOS asks once ("Sidevoice quiere acceder al micrófono"). The answer is stored per app
signature: with ad-hoc signing each new build is a "new" app and may ask again.
If it was denied: System Settings → Privacy & Security → Microphone, or
`tccutil reset Microphone dev.sidevoice.desktop`.

Echo cancellation is the web UI's own (`echoCancellation` constraints, `'all'` where supported);
WKWebView implements it with the system voice-processing unit.

## Sign-in

The operator's room sits behind oauth2-proxy with Google sign-in. Google refuses sign-in from
browsers it takes for an embedded webview, and WKWebView's default user agent lacks Safari's
`Version/… Safari/…` tokens. The room window therefore sends Safari's user agent
(`MAC_USER_AGENT` in `src/lib.rs`). The login cookie lives in the webview's persistent store and
survives restarts.

If Google ever blocks it anyway, the fallback is to sign in with a flow that ends in the app —
e.g. the room issuing a device token via pairing (the architecture's "auth = pairing"), not a
browser cookie. Not built.

## Background behaviour

- Closing the window hides it; the call continues. The app stays in the menu bar and the Dock.
- The room webview has background throttling **disabled** (`BackgroundThrottlingPolicy::Disabled`,
  macOS 14+), so a hidden window keeps capturing and playing. On macOS 13 WebKit may throttle
  timers of a hidden page; audio capture continues.
- Quit: tray → "Salir de Sidevoice" or ⌘Q.

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
5. Auto-update (`tauri-plugin-updater`) needs signed builds plus an update signing key; later.

## iPhone (out of scope)

Tauri 2 builds iOS apps, but: an Apple Developer account and provisioning profiles; the WKWebView
code path is the same, but background audio needs the `audio` background mode and an
`AVAudioSession` category set natively; no tray or global shortcut, so the bridge's commands map
to lock-screen / Control Center controls (`MPRemoteCommandCenter`) instead; distribution via
TestFlight.
