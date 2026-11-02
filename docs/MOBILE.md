# The mobile app

`mobile/` is the Sidevoice app for phones: a Tauri 2 shell (`sidevoice-mobile`, identifier `ai.sidevoice.mobile` on iOS
and Android) around the same web interface the desktop app bundles. It is a crate of its own beside `desktop/`: a kind
of host is a crate, never a condition (AGENTS.md → "Host kind is a crate boundary"). Plan: sidevoice/sidevoice-desktop#289.

## What it is today

- **The window** (`mobile/tauri/src/lib.rs`) shows `voice/index.html`, the interface of the sidevoice-web release
  `web.pin.json` names (`npm run web` puts it in `ui/`, as for the desktop app, and every Tauri build and dev run starts
  with it).
- **The bridge** gives the page the app's members (`window.__sidevoiceHost`; "The bridge" below): the voice call the app
  runs, its engine, the microphone's permission and the pairings, through the commands every app shares.
- **No page captures anything.** The permission handler denies every request (microphone, camera, screen): the webview
  would grant them all if the app set none. The app owns the microphone and the speaker, as the desktop app does.
- **Only the app's own pages.** A navigation anywhere else goes nowhere (`navigation_allowed`).
- **The origin** the page presents is Tauri's own: `http://tauri.localhost` on Android, `tauri://localhost` on iOS
  (`sidevoice-shared-tauri`'s `platform/origin.rs`, the one the desktop app reads too). The node accepts both without configuration (docs/TARGETS.md). On Android the page
  is on http, so reaching a node's plain-http address on the local network is not mixed content.
- **Plain http to the local network.** A node on the local network has an address no certificate can name, so the app
  may speak plain http to it. On Android that is a network security config in `mobile/tauri/plugin/android`, merged into the
  app's manifest (the template's release build says `usesCleartextTraffic=false`, which a network security config
  overrides). It permits cleartext for **every** destination: the config names hosts, not private address ranges, and a
  pairing code carries whatever address the machine has. What keeps the app off public http is the page's own reach
  policy (it refuses a public address over http) and that the only page is the bundled one. On iOS it is
  `NSAllowsLocalNetworking` (`mobile/tauri/Info.ios.plist`), which allows local addresses and leaves App Transport Security on
  for everything else.
- **No pairings in backups.** The pairings hold the access token to the person's computer, so they are excluded from
  every OS backup and device transfer (operator decision, 2026-11-01), like the provider keys: after a restore the person
  pairs again. The record is `pairings/pairings.json` in the app's files, alone in its folder, so one folder is what is kept
  out, the temporary file of an interrupted write included. iOS marks `pairings/` `isExcludedFromBackup` before each
  write (`mobile/tauri/src/platform/backup/ios.rs`). Android excludes `pairings/`, and the SharedPreferences file of the provider
  keys (`keyring-sidevoice-providers.xml`, which another phone could not decrypt anyway), in the plugin's
  `res/xml/data_extraction_rules.xml` (Android 12 and newer: cloud backup and device transfer) and `res/xml/backup_rules.xml`
  (Android 11 and older), which its manifest points to. The details are in docs/PAIRING_STORAGE.md ("On the phone" →
  Backups).
- **Permission texts.** `NSMicrophoneUsageDescription` and `NSLocalNetworkUsageDescription` are in
  `mobile/tauri/Info.ios.plist` in English, the base (App Store validation looks for them there), and in the languages the web
  interface ships (de, es, fr, it, ja, pl, zh-Hans) in `mobile/tauri/ios/<language>.lproj/InfoPlist.strings`, which `generate`
  puts into the iOS project. `CFBundleLocalizations` in the plist is the list of languages iOS offers the app in. Adding
  a language is a folder in `mobile/tauri/ios` and an entry in that list; a test checks that they agree, and that the English
  of `en.lproj` is the plist's.
- **What the iOS app declares to Apple** (`mobile/tauri/PrivacyInfo.xcprivacy` and `mobile/tauri/Info.ios.plist`; the numbers below
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

## The bridge

`mobile/tauri/src/bridge.rs` composes the page's script as the desktop does (docs/BRIDGE.md → "Parts"): the transport of the
shared plugin (`sidevoice-shared-tauri`), the members every app shares (`sidevoice-shared-tauri/js/members.js`) and the app's own
part, `mobile/tauri/js/mobile-bridge.js`, which assembles `host`:

| `host.*` | What |
|---|---|
| `voice`, `engine` | The voice call the app runs itself (sidevoice-voice on the phone's microphone and speaker) and the engine it takes its models from: sherpa-onnx (Silero) and the remote providers on a phone (sidevoice-engine's README, "Android" and "iOS"). |
| `microphone` | The system's own permission, never the webview's: AVFoundation's on iOS, the runtime permission on Android (`sidevoice-shared-tauri`'s `platform/microphone`). `openSettings` opens the app's page of the system's settings, and `help(language)` gives the app's own words for a blocked microphone on this phone (`mobile/tauri/src/i18n.rs`, en and es, English the fallback; D-HOST-OS). |
| `pairings` | The pairings in the app's private storage (`mobile/tauri/src/pairings.rs`), not the webview's. |
| `presence` | `{ media: "app" }`: the app owns the call's audio, so the page registers no Media Session and plays no Now Playing element of its own. The app answers the headset's play/pause and the lock screen's (the shared plugin's `platform/media_controls`: MediaPlayer on iOS, a `MediaSession` held by the call's service on Android; docs/MEDIA_CONTROLS.md). |
| `reach` | `{ plainHttpPrivate: true }`, as on the desktop. |

Nothing of the desktop's own (`localHost`, `reset`, `muteShortcut`, `callControls`, `updater`, `debug`, `deviceName`) is
offered, and neither is `build` yet: the mobile app's build kinds (nightly and release, D-MOBILE-CHANNELS) come with their
own row. The page reports the call (the members' `room`) to the app's own `call_state`, and the app runs commands on the room
through `run` (the call's service hangs up through it).

- **Who may call.** The app's one window, `main`, on the app's own origin (`is_the_page`), as every shared command asks
  first; `mobile/tauri/capabilities/main.json` grants it every command of `shared/tauri/ACL.md` but the call's levels
  (`bridge_level`, `bridge_playback`), which feed the desktop's call controls card, and the app's own `call_state`. A
  test keeps the two in step.
- **The application host** (`AppHost`) is the desktop's, with the phone's hooks: every request admitted (a phone never
  resets its data under the call), the provider keys under `ai.sidevoice.mobile.providers` (docs/CREDENTIALS.md), and the
  models in `models/` of the app's own files, private to the app and kept out of the device backup (iOS marks the
  folder, Android's backup rules name it). A page that loads anew owns none of the calls of the one before; quitting stops
  the calls and lets the models go, as on the desktop.
- **The call's lifecycle** (D-CALL-LIFECYCLE, `mobile/tauri/src/call.rs`), the same on both phones: the green call button
  starts the call, the red one ends it.
  - **The room's call** holds the call's foreground service (Android): the page reports the call to the app's own
    `call_state`, with its page's number (asked once from `call_page`, so a report of a replaced page that reaches the
    app late changes nothing), and one thread of the app (`RoomCall`) takes the reports, the page changes and the
    service's own end in order. The service starts when the report says joined (a press of the green button, with the app in
    front, as Android requires) and stops when it says not (the red button, a page that loads anew, the app quitting).
    A start the system refuses is logged by its code (`not-visible`, `microphone-permission`, …), and the call goes on in
    front. The service ending on its own (Hang up in its notification, the app removed from the recents list, the
    system) hangs up the call on the page (`window.__sidevoiceHost.run({command: "hang-up"})`). The media controls follow `call_state` too, and the
    call's service owns the Android media session.
  - **The call's audio** holds the audio session (iOS): sidevoice-shared's host tells the app before the first voice
    call's audio opens and after the last one's has stopped (`Hooks::calls`, `CallActivity`; the idle state that answers
    a stop, which sidevoice-voice sends once its audio is closed; the desktop does nothing there). Every voice call
    counts, a try in Settings too. A session the system will not activate refuses the call with its code
    (`audio-session-activate-failed`).
- **The audio session** (iOS; D-AUDIO-SESSION, sidevoice-voice's `Session`): declared at the app's start as
  play-and-record with mode `Default`, the loudspeaker rather than the receiver, and Bluetooth hands-free allowed;
  activated only when a call starts (above). Android has no session to declare. **Not verified on a device yet**; see
  "Not verified" below.
- **Background audio** (iOS): `UIBackgroundModes` `audio` in `mobile/tauri/Info.ios.plist`, so a call goes on with the screen
  locked. Android keeps the call's process with its foreground service (above), which runs for exactly the call.
- **HTTPS on Android.** The engine's `reqwest` verifies certificates with `rustls-platform-verifier`, whose Android side is
  Kotlin: the Android library of `mobile/tauri/plugin` adds it to the app's Gradle build (from the crate's own Maven directory,
  which `cargo metadata` finds, at the version Cargo.lock resolves; `consumer-rules.pro` keeps it from R8), and the app
  hands the verifier the application's context at start (`mobile/tauri/src/platform/tls/android.rs`). `ndk_context`, which
  cpal and the provider keys need, is set by Tauri's runtime (tao) before the app's code runs: nothing to do for it.
- **The engine's native libraries.** sherpa-onnx is shared on a phone: `libsherpa-onnx-c-api.so` and `libonnxruntime.so`
  per ABI on Android (in `jniLibs/<abi>/`), `sherpa-onnx.xcframework` and `onnxruntime.xcframework` on iOS (one shared ONNX Runtime, for sherpa-onnx and the smart-turn backend; embedded and signed through `bundle.iOS.frameworks`
  in `mobile/tauri/tauri.conf.json`, beside the system frameworks the Rust library calls and Xcode must link: AVFoundation and
  AVFAudio for the audio session and the microphone's permission, AudioToolbox for cpal's audio units). `cargo xtask mobile build` gets them from the engine's own `cargo xtask mobile-libs android|ios` (no engine build), at the commit Cargo.lock pins (the archives its pins name, checked against their digests), and builds the
  app against the same directories (`SHERPA_ONNX_LIB_DIR`, and on iOS `ONNXRUNTIME_FRAMEWORK_DIR`), so nothing is downloaded unchecked. Both variables are also
  written into the repository's `.cargo/config.toml` (`[env]`) for the length of the build and taken out after it: the
  cargo the native project runs again from inside its build (Xcode's build phase, Gradle's `rust` task) gets from the
  Tauri CLI only the `TAURI*`, `WRY*`, `CARGO_*` and `RUST_*` variables, and would otherwise link a copy of its own. Git ignores both
  (`mobile/tauri/frameworks/`, the `.so` files of `jniLibs`).
- **The log**: `tauri-plugin-log` to the system's (logcat, the unified log) and a file in the app's log folder.

### Not verified

What only a device shows, for the M-FG gate (plan §F.2): a call's first Join on each phone (remote STT/TTS and Silero, a
turn and a reply); the audio session's route (loudspeaker, a wired or Bluetooth headset) and what other apps' audio does
when a call starts and ends (the session is activated and deactivated with it); the call's service starting and
stopping with the call, and its Hang up; the media controls (a headset's play/pause with and without headphones,
the lock screen and Control Center on iOS, the call notification's controls on Android); that a model download and a remote provider's request
pass the verifier on Android; that the xcframework is embedded and signed in an iOS archive; and the APK's libraries
loading on a phone.

## Who owns the native projects

The generator model is **G-a: repeatable generation** (design D.4). `mobile/tauri/gen/android` and `mobile/tauri/gen/apple` are
what the Tauri CLI writes, are committed, and are **never edited by hand**. Whatever the app needs beyond the template
is an input the CLI reads:

| What | Input |
|---|---|
| Identifier, name, version, minimum OS versions, the web interface to bundle | `mobile/tauri/tauri.conf.json` |
| iOS `Info.plist` keys (permission texts, export compliance) | `mobile/tauri/Info.ios.plist` (merged by the CLI) |
| The iOS privacy manifest | `mobile/tauri/PrivacyInfo.xcprivacy`: the CLI has no setting for it, so `generate` puts it in `gen/apple/sidevoice-mobile_iOS/` before the CLI makes the project, and the project lists it as a resource of the app (`generate` fails if it does not) |
| The iOS permission texts in each language | `mobile/tauri/ios/<language>.lproj/InfoPlist.strings`: the CLI has no setting for them, so `generate` puts them in `gen/apple/sidevoice-mobile_iOS/` before the CLI makes the project, and the project lists them as resources of the app, with each language as a known region (`generate` fails if it does not) |
| Android manifest entries, resources and, later, Kotlin | the Android library of `mobile/tauri/plugin` (a Tauri plugin carrying only native project inputs; Gradle merges its manifest into the app's) |
| App icons | `mobile/tauri/app-icon/` (drawn from the brand by `npm run icons`; `tauri icon` derives every size) |
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
unless a foreground service says it is in use. `mobile/tauri/plugin` carries that service (`CallService.kt`, `MobilePlugin.kt`
and the manifest entry, all merged into the app by Gradle). It has the foreground service types **microphone** (the call
listens) and **media playback** (the agent speaks), and a notification with the app's name, "In a call" and a **Hang up**
action, in the system's language (`res/values`, English, and `values-es`; the Spanish is the desktop app's wording).

- **Start while visible.** Android lets a microphone service start only from an app that is on screen and has the
  microphone allowed (since Android 14, and enforced strictly from 15), so `start_call_service` is called while the app is
  visible. Visible is the hosting activity's own lifecycle (started), not the process's, which reports a stop some time
  late. The call refuses by name (`not-visible`, `microphone-permission`, `already-running`, `not-allowed`) instead
  of by the system's exception. Once the service runs, it goes on in the background.
- **A deterministic end.** One service at a time, and one `Stopped` report to the app for each start, whoever ends it. Each
  start is a session with a number, which the service carries in its intents; the system ends a service after the app may
  have started the next call, so a service only acts on its own session (its end, its focus, its notification's Hang up
  are nothing once that session is over). `cargo xtask mobile test android` runs the JVM tests of that rule. Whoever ends it:
  the app (`stop_call_service`: `requested`), the person (Hang up in the notification: `hang-up`; the app removed from
  the recents list: `task-removed`) or the system (`system`, also when it refused the start). The notification, the
  audio focus and the service go with it, and the system never restarts it: a call does not survive its process.
- **Audio focus.** The service holds the call's focus (transient: other media pauses and comes back after) from the start
  to the end, and reports `Focus` changes (`gained`, `lost`, `lost-transient`, `duck`, `refused`). What to do about them (mute,
  pause, hang up) is the call's decision, not the service's.
- The Rust side is `start_call_service(app, on_event)` and `stop_call_service(app)` of `sidevoice-mobile-plugin`
  (`call_service.rs`); the shared call code reaches the service through them. iOS has no service to start: a call stays
  alive there through the `audio` background mode, which is configuration.
- The manifest declares `RECORD_AUDIO`, the two foreground-service permissions and `POST_NOTIFICATIONS`; asking the
  person for the microphone and for notifications is the microphone permission's row. Without notifications allowed
  (Android 13 and later) the service runs and the system lists it, but shows no notification.

## Building

Always one package, never the workspace: it holds the desktop app too (and the mobile crates are empty on a computer: `#![cfg(mobile)]` at their roots, so a plain `cargo` command there skips them; `cargo xtask mobile lint android|ios` runs Clippy over them for their target). The Tauri CLI is told which app is the one
(`TAURI_APP_PATH`): Gradle calls it back from the package root, where it would find `desktop/tauri/` as well.

```
# Android: the SDK (platform 37), an NDK (r28 or later: 16 KB pages), JDK 21 and the Rust targets; from mobile/tauri/
TAURI_APP_PATH=$PWD npx tauri android build --apk --target aarch64 --ci

# iOS: macOS with Xcode and XcodeGen
TAURI_APP_PATH=$PWD npx tauri ios build --target aarch64-sim --no-sign --ci
```

A build by hand needs the engine's native libraries first (`cargo xtask mobile-libs` in the engine), and `SHERPA_ONNX_LIB_DIR` (on iOS also `ONNXRUNTIME_FRAMEWORK_DIR`) pointing at the target's (above,
"The bridge"); `cargo xtask mobile build android|ios` does both (it needs no cargo-ndk).

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
  wrote, the job uploads what it wrote (`mobile-gen-apple`) to commit as `mobile/tauri/gen/apple`.
- The iOS build ends with `cargo xtask mobile check-ios` on the app it made: the privacy manifest is at the root of the
  bundle, the encryption and tracking declarations are there, and the manifest gives a reason for each required-reason API
  the executable calls (above).
- **`generated`** is the regeneration diff: it runs `generate` and fails on any difference in `mobile/tauri/gen/<project>`.
- Every external action is pinned by its full commit SHA, the version in a comment, so a moved tag cannot run new code
  beside the build; so are those of the composite action and of the signing workflow. (The desktop workflows still use
  tags.)
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

## TestFlight

`.github/workflows/mobile-testflight.yml` signs the iOS app and uploads it to TestFlight. It is run by hand (Actions →
mobile-testflight → Run workflow, on `main`) and runs nothing else: no pull request, no push, and every other mobile build
stays unsigned with no secret. Its one step is `cargo xtask mobile testflight --build-number <run number>`
(`xtask/src/mobile/testflight.rs`):

1. The Tauri CLI archives the committed project and exports it for App Store Connect. Signing is **Xcode's automatic
   provisioning**: `xcodebuild -allowProvisioningUpdates` with the App Store Connect API key
   (`-authenticationKeyPath`, `-authenticationKeyID`, `-authenticationKeyIssuerID`), which gets the Apple Distribution
   certificate (managed by Apple, not imported into a keychain) and the App Store provisioning profile for
   `ai.sidevoice.mobile`, and registers that identifier in the account if it is not there. No certificate or profile is
   stored as a secret.
2. The archive is read before anything leaves the runner: it is the app `ai.sidevoice.mobile` at the build number, it is
   signed, and it passes the privacy and export-compliance check (above).
3. `xcrun altool --upload-app` uploads it. App Store Connect processes it (minutes), and TestFlight offers it to the
   internal testers.

- **The build number** is the run number of the workflow, which only grows, as TestFlight requires of the builds of a
  version, and it is the app's `CFBundleVersion` whole. (The CLI's own `--build-number` appends it to the version,
  `0.1.0.7`: four numbers where Apple accepts three.) The version, `CFBundleShortVersionString`, is `tauri.conf.json`'s.
  A re-run of a run whose upload App Store Connect accepted has the same number and fails: dispatch again (a new run, a
  new number) rather than re-running.
- **Credentials.** The repository secrets `APPLE_API_KEY` (the key's id), `APPLE_API_ISSUER`, `APPLE_API_KEY_P8` (the
  .p8's contents) and `APPLE_TEAM_ID`, the same the desktop notarization uses; the key needs the **Admin** role in App
  Store Connect (the Developer role cannot create the certificate or the profile). Only the step that signs has them;
  the .p8 is written to a file only the job's user can read, in a directory of its own, and removed when the command ends;
  the contents are taken out of the command's environment as they are read, so no process it starts (the Tauri CLI,
  `xcodebuild`, `altool`) gets them, only the file's path and the key's id and issuer. The checkout keeps no token.
- **Main only.** The job refuses any other ref (`if`), so does the command, and the job runs in the environment
  `testflight`. The gate that a workflow file on another branch cannot lift is the environment's: its deployment
  branches are `main` only (Settings → Environments), and the key's secrets can be moved into it.
- **Before the first dispatch**, in App Store Connect and the repository, once:
  1. the key's role is Admin (a new key replaces `APPLE_API_KEY`, `APPLE_API_ISSUER` and `APPLE_API_KEY_P8`; the desktop
     notarization keeps working with an Admin key);
  2. any pending agreement is accepted (App Store Connect → Business → Agreements, by the account holder);
  3. the app exists in App Store Connect (Apps → New App): platform iOS, bundle identifier `ai.sidevoice.mobile`, a name
     and an SKU. Apple's API reference has no call that creates an app record (it can register a bundle identifier, not
     this), so it is by hand. The identifier must be registered before the
     form lists it: either by hand (Developer account → Identifiers → App IDs, explicit `ai.sidevoice.mobile`) or by a first
     dispatch, whose export registers it and whose upload then fails for lack of the record;
  4. the people who test are users of the team (Users and Access) and members of an internal testing group of the app
     (TestFlight → Internal Testing); an internal build needs no beta review;
  5. the environment `testflight` exists with the deployment-branch rule.

## Audio interruptions (D-AUDIO-FOCUS)

A phone call, an alarm or another app taking the audio **pauses the conversation while it lasts and resumes it when it
ends**; it never hangs up on its own (`mobile/tauri/src/call.rs`). Only while the room's call is held (a try in Settings is not
paused).

- **Where it comes from.** iOS: `AVAudioSessionInterruptionNotification` (`platform/interruption`), `began` and `ended`
  with `shouldResume`. Android: the call's service reports its audio focus (`Focus`): `lost` and `lost-transient` begin an
  interruption that ends with `gained`; `lost` (for good: another app plays) and `refused` (the system refused the request
  outright) pause with no end to wait for; `duck` (another app quietly over the call) is none. A request made while a phone call is on is delayed by the system:
  `lost-transient` now, `gained` when the call ends. The call pauses at the first interruption and resumes at the last end,
  whichever source.
- **Pause** is the page's own mute (the `interrupt` command of `mobile/tauri/js/mobile-bridge.js`, so the page shows the call
  muted) and the agent's playback held (`AppHost::pause_playback`: the chunks not yet handed to the speaker are kept, in
  order, and played on resuming). **Known limit:** what the speaker already has queued is the device's and keeps playing
  (iOS stops it with the session; on Android the system silences the media while a call is on); a lossless pause of that
  needs a pause in `sidevoice-voice`'s `AudioIo` (sidevoice-voice#94).
- **Resume** gives the mute back as it was, but only while it is still the system's: the page watches its mute, and a
  person who muted themselves before stays muted, one who unmuted (and perhaps muted again) has taken it over and is not
  touched. The commands carry the call they are for (`since`), so they never reach a later call of the same page; a call
  that ends while interrupted gives the mute back for the next one.
- **No end by itself**: `shouldResume` false (another app goes on playing), the audio session that could not be activated
  again, or a focus lost for good or refused (Android). The microphone stays muted and the playback held until the person
  unmutes: the session is asked for again first (refused again: the wait goes on), and on Android the service requests the audio
  focus again (`request_call_focus`), the playback is released only when it is granted; a delayed grant is a pause that ends
  with the gain, a refusal pauses again and the person must mute and unmute once more. The state machine is `mobile/tauri/src/hold.rs`.
- On iOS the audio session is activated again first (if a voice call has audio open), under the same lock as the session's
  activation and deactivation by the calls, so it is never active after the last call's audio has stopped.

## Downloads keep the screen on

The system suspends the app soon after the screen turns off, and a model download (the engine, inside the app's process) stops with it.
While any install job runs, the screen is kept on: the host tells the app when the first download starts and the last one ends
(`DownloadActivity`, one of `Hooks`), `mobile/tauri/src/downloads.rs` passes that to the plugin's `keep_screen_on`, which sets iOS
`UIApplication.isIdleTimerDisabled` (on the main thread) or Android's `FLAG_KEEP_SCREEN_ON` on the activity's window
(`MobilePlugin.keepScreenOn`). The engine resumes a partial file with an HTTP range request, so a download that the system
interrupts anyway (the person left the app) goes on where it stopped. Native background downloads are not here
(sidevoice-desktop#361, option 2).

## Not here yet

The build kinds (nightly with Developer options, release).
