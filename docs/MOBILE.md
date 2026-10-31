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
  may speak plain http to it: on Android through a network security config (`mobile/plugin/android`, merged into the
  app's manifest; the template's release build says `usesCleartextTraffic=false`, which a network security config
  overrides), on iOS through `NSAllowsLocalNetworking` (`mobile/Info.ios.plist`), which allows local addresses and
  leaves App Transport Security on for everything else.
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
iOS project needs macOS and XcodeGen. Run it after changing an input or the Tauri CLI's version, and commit the result;
CI regenerates the projects and fails on any difference.

The template sets `compileSdk` and `targetSdk` (37 with the CLI pinned in package.json); they are the template's, not
this repository's, and move with the CLI.

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
  release APK for arm64, as the artifact `Sidevoice-android-arm64-unsigned-apk`.
- **iOS** (macOS, XcodeGen): `cargo xtask mobile generated ios`, then `cargo xtask mobile build ios`, the app for the
  simulator, unsigned, as the artifact `Sidevoice-ios-simulator-app`. When the committed project is not what the CLI
  wrote, the job uploads what it wrote (`mobile-gen-apple`) to commit as `mobile/gen/apple`.
- **`generated`** is the regeneration diff: it runs `generate` and fails on any difference in `mobile/gen/<project>`.
- The Rust and Gradle caches are written by a push to `main` only and their keys start with `mobile`. The Android
  jobs share their setup (`.github/actions/setup-android`: JDK, the SDK platform and NDK versions, pinned there, the
  toolchain, the caches).

## Signing

`.github/workflows/mobile-android-signed.yml` signs the Android app with the upload key. It is run by hand
(`workflow_dispatch`), and its job refuses any ref but `main`; no pull request runs it. It builds the same unsigned APK
as above, then `cargo xtask mobile sign android` aligns it for 16 KB pages (`zipalign -P 16`), signs it (`apksigner`),
verifies the signature and checks that every native library's loadable segments are 16 KB aligned, which Google Play
requires (`cargo xtask mobile check-16kb [APK]` is that check alone). The result is the artifact
`Sidevoice-android-arm64-signed-apk`.

It reads three repository Actions secrets, which must exist:

| Secret | Holds |
|---|---|
| `ANDROID_UPLOAD_KEYSTORE_B64` | the upload key's PKCS#12 keystore, base64 encoded |
| `ANDROID_UPLOAD_KEYSTORE_PASSWORD` | its password |
| `ANDROID_UPLOAD_KEY_ALIAS` | the key's alias (`upload`) |

Only the signing step sees them. The keystore and the password go into files only the runner's user can read, removed
when the step ends, and the password never reaches a command line. On a laptop, `ANDROID_UPLOAD_KEYSTORE_PATH` names
the keystore file instead of `ANDROID_UPLOAD_KEYSTORE_B64`.

## Not here yet

The bridge (`__sidevoiceHost`: voice, engine, microphone, pairings), the microphone permission, `UIBackgroundModes`
(audio), the privacy manifest and export compliance, the Android foreground service, and the iOS signing.
