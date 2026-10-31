# The mobile app

`mobile/` is the Sidevoice app for phones: a Tauri 2 shell (`sidevoice-mobile`, identifier `ai.sidevoice.mobile` on iOS
and Android) around the same web interface the desktop app bundles. It is a crate of its own beside `desktop/`: a kind
of host is a crate, never a condition (AGENTS.md → "Host kind is a crate boundary"). Plan: sidevoice/sidevoice-desktop#289.

## What it is today

- **The window** (`mobile/src/lib.rs`) shows `voice/index.html`, the interface of the sidevoice-web release
  `web.pin.json` names (`npm run web` puts it in `ui/`, as for the desktop app, and every Tauri build and dev run starts
  with it).
- **No bridge.** The app injects nothing, so the interface runs as it does in a browser: it pairs with a machine through
  a one-time code and keeps its pairings in the webview's storage. The members the apps give it (`window.__sidevoiceHost`)
  come with later rows.
- **No page captures anything.** The permission handler denies every request (microphone, camera, screen): the webview
  would grant them all if the app set none. The app will own the microphone, as the desktop app does.
- **Only the app's own pages.** A navigation anywhere else goes nowhere (`navigation_allowed`).
- **The origin** the page presents is Tauri's own: `http://tauri.localhost` on Android, `tauri://localhost` on iOS
  (`mobile/src/platform/origin.rs`). The node accepts both without configuration (docs/TARGETS.md). On Android the page
  is on http, so reaching a node's plain-http address on the local network is not mixed content.
- **Plain http to the local network.** A node on the local network has an address no certificate can name, so the app
  may speak plain http to it. On Android that is a network security config in `mobile/plugin/android`, merged into the
  app's manifest (the template's release build says `usesCleartextTraffic=false`, which a network security config
  overrides). It permits cleartext for **every** destination: the config names hosts, not private address ranges, and a
  pairing code carries whatever address the machine has. What keeps the app off public http is the page's own reach
  policy (it refuses a public address over http) and that the only page is the bundled one. On iOS it is
  `NSAllowsLocalNetworking` (`mobile/Info.ios.plist`), which allows local addresses and leaves App Transport Security on
  for everything else.
- **Permission texts.** `NSMicrophoneUsageDescription` and `NSLocalNetworkUsageDescription` are in
  `mobile/Info.ios.plist`, in English; their translations are a later, reviewed change.

## Who owns the native projects

The generator model is **G-a: repeatable generation** (design D.4). `mobile/gen/android` and `mobile/gen/apple` are
what the Tauri CLI writes, are committed, and are **never edited by hand**. Whatever the app needs beyond the template
is an input the CLI reads:

| What | Input |
|---|---|
| Identifier, name, version, minimum OS versions, the web interface to bundle | `mobile/tauri.conf.json` |
| iOS `Info.plist` keys | `mobile/Info.ios.plist` (merged by the CLI) |
| Android manifest entries, resources and, later, Kotlin | the Android library of `mobile/plugin` (a Tauri plugin carrying only native project inputs; Gradle merges its manifest into the app's) |
| App icons | `mobile/app-icon/` (drawn from the brand by `npm run icons`; `tauri icon` derives every size) |
| The Rust toolchain and the four mobile targets | `mobile/rust-toolchain.toml` |

`cargo xtask mobile generate android|ios` makes a project anew (the old one removed) and derives the icons into it. The
iOS project needs macOS and XcodeGen. Run it after changing an input or the Tauri CLI's version, and commit the result.
An iOS build merges `Info.ios.plist` into
`gen/apple/sidevoice-mobile_iOS/Info.plist`, so a local iOS build changes that one file; `generate` restores it.

`compileSdk` is the template's (37 with the CLI pinned in package.json) and moves with the CLI. **`targetSdk` is 36**, the
plan's: the template writes 37 and the CLI has no setting for it, so `generate` writes 36 over that one line (and fails
if the template stops having it). Android 17 (API 37) asks an app that targets it to be allowed the local network
(`ACCESS_LOCAL_NETWORK`, a runtime permission) before it reaches a machine on it, and the app declares and requests
nothing of the kind yet; moving `targetSdk` to 37 comes with that permission, in the manifest and as a request before the
first connection to a paired machine.

## The call's service (Android)

A call must outlive the screen: with it off, or another app in front, Android stops a process that holds the microphone
unless a foreground service says it is in use. `mobile/plugin` carries that service (`CallService.kt`, `MobilePlugin.kt`
and the manifest entry, all merged into the app by Gradle). It has the foreground service types **microphone** (the call
listens) and **media playback** (the agent speaks), and a notification with the app's name, "In a call" and a **Hang up**
action, in the system's language (`res/values`, English, and `values-es`; the Spanish is the desktop app's wording).

- **Start while visible.** Android lets a microphone service start only from an app that is on screen and has the
  microphone allowed (since Android 14, and enforced strictly from 15), so `start_call_service` is called while the app is
  visible and the call refuses by name (`not-visible`, `microphone-permission`, `already-running`, `not-allowed`) instead
  of by the system's exception. Once the service runs, it goes on in the background.
- **A deterministic end.** One service at a time, and one `Stopped` report to the app for each start, whoever ends it:
  the app (`stop_call_service`: `requested`), the person (Hang up in the notification: `hang-up`; the app removed from
  the recents list: `task-removed`) or the system (`system`, also when it refused the start). The notification, the
  audio focus and the service go with it, and the system never restarts it: a call does not survive its process.
- **Audio focus.** The service holds the call's focus (transient: other media pauses and comes back after) from the start
  to the end, and reports `Focus` changes (`gained`, `lost`, `lost-transient`, `duck`). What to do about them (mute,
  pause, hang up) is the call's decision, not the service's.
- The Rust side is `start_call_service(app, on_event)` and `stop_call_service(app)` of `sidevoice-mobile-plugin`
  (`call_service.rs`); the shared call code reaches the service through them. iOS has no service to start: a call stays
  alive there through the `audio` background mode, which is configuration.
- The manifest declares `RECORD_AUDIO`, the two foreground-service permissions and `POST_NOTIFICATIONS`; asking the
  person for the microphone and for notifications is the microphone permission's row. Without notifications allowed
  (Android 13 and later) the service runs and the system lists it, but shows no notification.

## Building

Always one package, never the workspace: it holds the desktop app too. The Tauri CLI is told which app is the one
(`TAURI_APP_PATH`): Gradle calls it back from the package root, where it would find `desktop/` as well.

```
# Android: the SDK (platform 37), an NDK (r28 or later: 16 KB pages), JDK 21 and the Rust targets; from mobile/
TAURI_APP_PATH=$PWD npx tauri android build --apk --target aarch64 --ci

# iOS: macOS with Xcode and XcodeGen
TAURI_APP_PATH=$PWD npx tauri ios build --target aarch64-sim --no-sign --ci
```

An Android build without a signing key is an unsigned APK; a debug build (`--debug`) is signed with the debug key and
installs as it is.

## Not here yet

The bridge (`__sidevoiceHost`: voice, engine, microphone, pairings), the microphone permission, `UIBackgroundModes`
(audio), the privacy manifest and export compliance, and the CI and signing of both apps.
