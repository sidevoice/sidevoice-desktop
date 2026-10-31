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
- **What the iOS app declares to Apple** (`mobile/PrivacyInfo.xcprivacy` and `mobile/Info.ios.plist`; the numbers below
  are the app as of the Tauri shell alone, and each later row that links more code re-reads them):
  - *Tracking and collection: none.* `NSPrivacyTracking` is false, with no tracking domains, and no data type is
    collected: the app has no server of its own, no analytics and no account, and the only place a pairing code or a
    page's traffic goes is the person's own machine. When the app starts to send audio or text to a provider the person
    chose (the first native call), whether that is collection to declare is a decision for the operator, in that row.
  - *Required-reason APIs: file timestamp only.* The executable imports `stat` and `fstat` (Rust's `std::fs`, which
    Tauri reads its bundled files and its data directory with), declared with reason `C617.1` (metadata of files in the
    app's own container). It calls nothing of the other categories: no system boot time (`mach_absolute_time`,
    `systemUptime`), no disk space (`statfs`, `statvfs`, the volume capacity keys), no user defaults, no active
    keyboards. `cargo xtask mobile build ios` checks it on every build: the executable's strings are read for the names of
    those APIs, and a call the manifest gives no reason for fails the build, so a library that brings one in is caught
    where it lands.
  - *Export compliance: no encryption beyond the system's.* `ITSAppUsesNonExemptEncryption` is false. What the app has is
    the system's TLS (the web view's own network stack) and hashing (SHA-256: `sha2` in the Rust libraries, `crypto.subtle
    .digest` and wasm in the web interface), none of which is encryption of the app's own. Native code that brings its own
    TLS or cipher (a Rust TLS library, an AEAD) changes the answer to true, with Apple's annual self-classification
    report; that is checked in the row that adds it.

## Who owns the native projects

The generator model is **G-a: repeatable generation** (design D.4). `mobile/gen/android` and `mobile/gen/apple` are
what the Tauri CLI writes, are committed, and are **never edited by hand**. Whatever the app needs beyond the template
is an input the CLI reads:

| What | Input |
|---|---|
| Identifier, name, version, minimum OS versions, the web interface to bundle | `mobile/tauri.conf.json` |
| iOS `Info.plist` keys (permission texts, export compliance) | `mobile/Info.ios.plist` (merged by the CLI) |
| The iOS privacy manifest | `mobile/PrivacyInfo.xcprivacy`: the CLI has no setting for it, so `generate` puts it in `gen/apple/sidevoice-mobile_iOS/` before the CLI makes the project, and the project lists it as a resource of the app (`generate` fails if it does not) |
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

## CI

`.github/workflows/mobile.yml` runs on every pull request (and on a push to `main`), on its own, so the desktop app's
`build.yml` neither waits for it nor is held up by it. No job signs anything or sees a secret.

- **Android** (Ubuntu): `cargo xtask mobile generated android`, then `cargo xtask mobile build android`, the unsigned
  release APK for arm64, as the artifact `Sidevoice-android-arm64-unsigned-apk`. The build ends with the 16 KB check Google
  Play makes (`cargo xtask mobile check-16kb [APK]`): the APK's uncompressed libraries start on 16 KB boundaries
  (`zipalign -c -P 16`) and every loadable segment of every library is aligned to 16 KB (the ELF program headers); it
  fails on any other and when there is no library. The job's log lists each library with its alignment.
- **iOS** (macOS, XcodeGen): `cargo xtask mobile generated ios`, then `cargo xtask mobile build ios`, the app for the
  simulator, unsigned, as the artifact `Sidevoice-ios-simulator-app`. When the committed project is not what the CLI
  wrote, the job uploads what it wrote (`mobile-gen-apple`) to commit as `mobile/gen/apple`.
- The iOS build ends with `cargo xtask mobile check-ios` on the app it made: the privacy manifest is at the root of the
  bundle, the encryption and tracking declarations are there, and the manifest gives a reason for each required-reason API
  the executable calls (above).
- **`generated`** is the regeneration diff: it runs `generate` and fails on any difference in `mobile/gen/<project>`.
- Every external action is pinned by its full commit SHA, the version in a comment, so a moved tag cannot run new code
  beside the build. (The desktop workflows still use tags.)
- The Rust and Gradle caches are written by a push to `main` only and their keys start with `mobile`. The Android SDK
  platform and NDK versions are pinned in the workflow's `env`.

## Not here yet

The bridge (`__sidevoiceHost`: voice, engine, microphone, pairings), the microphone permission, `UIBackgroundModes`
(audio), the Android foreground service, and the signing of both apps.
