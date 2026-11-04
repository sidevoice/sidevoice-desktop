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
  would grant them all if the app set none. The app owns the microphone and the speaker, as the desktop app does; the one
  camera is the app's scanner of the pairing code ("Scanning the pairing code" below), native, never the webview's.
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
- **Permission texts.** `NSMicrophoneUsageDescription`, `NSLocalNetworkUsageDescription` and `NSCameraUsageDescription` (the pairing code's scanner, below) are in
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
| `deviceName` | The name the person gave the phone (`mobile/tauri/src/platform/device_name`), read when the window is made: on Android `Settings.Global` `device_name`, else `Settings.Secure` `bluetooth_name`, else the model (`Samsung SM-A600FN`), with no permission; on iOS `UIDevice.current.name`, which since iOS 16 is the generic model ("iPhone") without the user-assigned-device-name entitlement. Without it the page would name the phone from the webview's user agent ("Chrome on Android"). |
| `debug` | The developer options of a nightly or a dev build (docs/BRIDGE.md → "The developer options"); absent in a release. One switch, `systemCall` ("Treat calls as system calls", off by default: "The system's call" below), kept in `developer-options.json` of the app's own files and set through the app's own `developer_options` command (`mobile/tauri/src/developer_options.rs`). |
| `build` | What this build is, in the desktop's shape (docs/BRIDGE.md → "The build"): `{channel, version, commit?, date?}`. `cargo xtask mobile build` and `testflight` tell the build script (`mobile/tauri/build.rs`) the channel: `nightly` on CI, so every build shipped today (TestFlight, the signed APK) is a nightly with its `commit` and `date`, `dev` on a laptop, `SIDEVOICE_BUILD_CHANNEL` overriding; `version` is `tauri.conf.json`'s. The page unlocks its Developer options by the channel (Settings → Advanced, the Build row tapped 10 times; nightly only). |
| `codeScanner` | `{ permission(), request(), scan(), cancel(), openSettings() }`: the camera that scans the pairing QR code a machine shows ("Scanning the pairing code" below). |
| `callSurface` | `{ active() }`, always `true`: the app draws the call's own surfaces (iOS's Live Activity, Android's call notification) from the room's `callSurface`, so the page publishes what costs it work (the creatures' stills, the spoken page). "The call surface" below. |

Nothing of the desktop's own (`localHost`, `reset`, `muteShortcut`, `callControls`, `updater`) is
offered. The page reports the call (the members' `room`) to the app's own `call_state`, and the app runs commands on the room
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
  - **The room's call** is the app's own call, and a phone call of the system's own only while the nightly developer option
    "Treat calls as system calls" is on ("The system's call" below: a CallKit call on iOS, the call's foreground service and its
    Telecom call on Android; off, Android's foreground service without Telecom, and on iOS the audio session alone): the page reports the call to the app's own
    `call_state`, with its page's number (asked once from `call_page`, so a report of a replaced page that reaches the
    app late changes nothing), and one thread of the app (`RoomCall`) takes the reports, the page changes and the
    service's own end in order. The service starts when the report says joined (a press of the green button, with the app in
    front, as Android requires) and stops when it says not (the red button, a page that loads anew, the app quitting).
    A start the system refuses is logged by its code (`not-visible`, `microphone-permission`, …), and the call goes on in
    front. The service ending on its own (Hang up in its notification, the app removed from the recents list, the
    system) hangs up the call on the page (`window.__sidevoiceHost.run({command: "hang-up"})`). The media controls follow `call_state` too, and the
    call's service owns the Android media session; on iOS they show nothing while the CallKit call lasts.
  - **The call's audio** holds the audio session (iOS): sidevoice-shared's host tells the app before the first voice
    call's audio opens and after the last one's has stopped (`Hooks::calls`, `CallActivity`; the idle state that answers
    a stop, which sidevoice-voice sends once its audio is closed; the desktop does nothing there). Every voice call
    counts, a try in Settings too. A session the system will not activate refuses the call with its code
    (`audio-session-activate-failed`). While the room's call is a CallKit call, CallKit activates and deactivates the
    session, not the app: the first voice call's audio waits for `provider(_:didActivate:)` (up to 5 s; past it, or while
    CallKit holds the call, the voice call is refused with `audio-session-activate-failed`: the app never activates a
    session CallKit owns), and the last one's end leaves the deactivation to CallKit. On Android the same wait holds
    for Telecom: setting the call active switches the audio mode to communication and the route to the loudspeaker, and
    AAudio cuts off any stream open across that (a spurious "audio device unavailable" at the call's start), so the
    first voice call's audio opens only once the call's service reports `audio`: Telecom made the call active and its
    route settled, or at once when Telecom refuses the call (it stays the app's own), or after 3 s whatever Telecom is
    doing. logcat (`SidevoiceCall`) says which: `audio: the call's audio opens now: …`.
- **The audio session** (iOS; D-AUDIO-SESSION, sidevoice-voice's `Session`): declared at the app's start as
  play-and-record with mode `Default`, the loudspeaker rather than the receiver, and Bluetooth hands-free allowed;
  activated only when a call starts (above). Android has no session to declare. **Not verified on a device yet**; see
  "Not verified" below.
- **Background audio** (iOS): `UIBackgroundModes` `audio` and `voip` in `mobile/tauri/Info.ios.plist`: `audio` so a call
  goes on with the screen locked, `voip` because CallKit refuses every call of an app without it (unentitled), and the call
  then has no call UI; `cargo xtask mobile check-ios` checks both on the app as built. Android keeps the call's process with its foreground service (above), which runs for exactly the call.
- **HTTPS on Android.** The engine's `reqwest` verifies certificates with `rustls-platform-verifier`, whose Android side is
  Kotlin: the Android library of `mobile/tauri/plugin` adds it to the app's Gradle build (from the crate's own Maven directory,
  which `cargo metadata` finds, at the version Cargo.lock resolves; `consumer-rules.pro` keeps it from R8), and the app
  hands the verifier the application's context at start (`mobile/tauri/src/platform/tls/android.rs`). `ndk_context`, which
  cpal and the provider keys need, is set by Tauri's runtime (tao) before the app's code runs: nothing to do for it.
- **The engine's native libraries.** sherpa-onnx is shared on a phone: `libsherpa-onnx-c-api.so` and `libonnxruntime.so`
  per ABI on Android (in `jniLibs/<abi>/`), `sherpa-onnx.xcframework` and `onnxruntime.xcframework` on iOS (one shared ONNX Runtime, for sherpa-onnx and the smart-turn backend; embedded and signed through `bundle.iOS.frameworks`
  in `mobile/tauri/tauri.conf.json`, beside the system frameworks the Rust library calls and Xcode must link: AVFoundation and
  AVFAudio for the audio session and the microphone's permission, AudioToolbox for cpal's audio units, Accelerate for whisper.cpp's CPU code; Metal and MetalKit, which whisper.cpp runs Whisper on, are linked by the Tauri template already). whisper.cpp itself is compiled into the Rust library from source by the engine (CMake on the PATH, as on macOS), its Metal shaders embedded in it: nothing more to bundle. `cargo xtask mobile build` gets them from the engine's own `cargo xtask mobile-libs android|ios` (no engine build), at the commit Cargo.lock pins (the archives its pins name, checked against their digests), and builds the
  app against the same directories (`SHERPA_ONNX_LIB_DIR`, and on iOS `ONNXRUNTIME_FRAMEWORK_DIR`), so nothing is downloaded unchecked. Both variables are also
  written into the repository's `.cargo/config.toml` (`[env]`) for the length of the build and taken out after it: the
  cargo the native project runs again from inside its build (Xcode's build phase, Gradle's `rust` task) gets from the
  Tauri CLI only the `TAURI*`, `WRY*`, `CARGO_*` and `RUST_*` variables, and would otherwise link a copy of its own. Git ignores both
  (`mobile/tauri/frameworks/`, the `.so` files of `jniLibs`).
- **The log**: `tauri-plugin-log` to the system's (logcat, the unified log) and a file in the app's log folder.
  - **Panics** (`mobile/tauri/src/panics.rs`): a hook logs each one (its thread, place and message) and adds it to
    `last-panic.txt` beside the log file, at most 16 KB; the next launch logs that file's panics as the last run's
    (`the last run panicked:`) and removes it. The hook never logs itself: a panic inside a log call holds the logger's
    lock, and logging from the hook would then deadlock (the standard hook still prints to standard error). It is
    installed once, first thing in `run_app`; the file's place is known once the log folder is (`setup`). A panic the app cannot unwind from aborts it before anything else is
    written, and the system's crash report has only the stack.

### Not verified

What only a device shows, for the M-FG gate (plan §F.2): a call's first Join on each phone (remote STT/TTS and Silero, a
turn and a reply); the audio session's route (loudspeaker, a wired or Bluetooth headset) and what other apps' audio does
when a call starts and ends (the session is activated and deactivated with it); the call's service starting and
stopping with the call, and its Hang up; the media controls (a headset's play/pause with and without headphones,
the lock screen and Control Center on iOS, the call notification's controls on Android); that a model download and a remote provider's request
pass the verifier on Android; that the xcframework is embedded and signed in an iOS archive; and the APK's libraries
loading on a phone. The pairing code's scanner: the camera prompt and a denial on each phone (and the Settings link after a
denial), a QR from a screen and from paper read in the page's viewfinder, Cancel and the system's back gesture during a scan, the web view
becoming opaque again after a scan, and a scan with the app sent to the background. The system's call: the CallKit call on the lock screen and in the in-call screen, its mute, end and hold, and the
audio starting only once CallKit activates the session; the Telecom call in a Bluetooth or car kit, its mute and
disconnect, a phone call holding it, the loudspeaker chosen, and what Telecom's communication mode does to the call's
audio (cpal's streams are not `VOICE_COMMUNICATION`, cpal#995).

## Scanning the pairing code

A phone pairs with a machine from the code the machine shows (`npx sidevoice pair-device`, or the agent's `voice_pair_device`): the
code (`SV1.…`) and, with it, the same text as a QR (sidevoice-connector `pairing.rs`, `qr_text`). The QR holds nothing but the
code, so scanning it is typing it: the page reads what the camera returns exactly as it reads a pasted code, and the typed path
stays as it was (sidevoice-desktop#405).

- **The scanner** is Tauri's official `tauri-plugin-barcode-scanner` (maintained by Tauri; CameraX with ML Kit's barcode model, delivered by Google Play services, on Android; AVFoundation on iOS), registered in `mobile/tauri/src/lib.rs`. Its commands are granted to the window one
  by one in `capabilities/main.json` (scan, cancel, check/request permissions, open the app's settings; not its vibration).
- **`host.codeScanner`** (`mobile/tauri/js/mobile-bridge.js`) is the page's one way to it: the plugin's permission states and
  rejection strings stay there, and the page gets `permission()` / `request()` (`granted | prompt | denied`), one `scan()` that
  never rejects (`scanned` with the text, `cancelled`, `denied`, `unavailable`: no camera, a simulator), `cancel()` and
  `openSettings()`. Only QR codes are looked for.
- **The viewfinder is the page's.** The scan is windowed: the camera is drawn behind the web view and the page shows it through
  (the plugin makes the web view transparent for the length of the scan), so the page draws the frame and its Cancel.
- **The camera permission** is the system's own, asked by the plugin when the person taps Scan (iOS: `NSCameraUsageDescription`, in
  the eight languages like the others; Android: the runtime `CAMERA` permission the plugin's manifest merges in, with its
  `VIBRATE`, which nothing calls). A denial is an answer the page words, with `openSettings()` where the system will not ask
  again; typing the code is always there.
- **No device is left out for lacking a camera.** The plugin's manifest declares `android.hardware.camera.any` (and asks for
  `CAMERA` and `VIBRATE`); the plugin of `mobile/tauri/plugin` replaces the feature as not required, adds `camera` and
  `camera.autofocus` as not required, and removes `VIBRATE`, which nothing calls. The plugin's scan never answers on an
  Android phone without a camera, so `scan()` first reads the web view's video inputs and answers `unavailable` when it
  finds none (a list it cannot read is left to the plugin; iOS refuses a missing camera itself).

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
| The Rust toolchain and the mobile targets | `mobile/rust-toolchain.toml` |

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
listens) and **media playback** (the agent speaks), and the call's notification, Sidevoice's own card ("The call surface"
below, "Android's card").

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
  until Telecom holds the call (then the focus is Telecom's: "The system's call" above), and reports `Focus` changes (`gained`, `lost`, `lost-transient`, `duck`, `refused`). What to do about them (mute,
  pause, hang up) is the call's decision, not the service's.
- The Rust side is `start_call_service(app, details, on_event)`, `update_call_service(app, details)` and
  `stop_call_service(app)` of `sidevoice-mobile-plugin` (`call_service.rs`); the app's call code reaches the service through
  them. On iOS the same functions run the CallKit call ("The system's call" above); the call stays alive there through
  CallKit and the `audio` and `voip` background modes.
- The manifest declares `RECORD_AUDIO`, the foreground-service permissions (microphone, media playback and phone call),
  the Telecom ones ("The system's call" above) and `POST_NOTIFICATIONS`; asking the
  person for the microphone and for notifications is the microphone permission's row. Without notifications allowed
  (Android 13 and later) the service runs and the system lists it, but shows no notification.

## The system's call (CallKit, Telecom)

**Off by default** (D-NO-CALLKIT-DEFAULT, operator 2026-11-02): the room's call is the app's own, with the system owning
neither the lock screen, nor the headset's buttons. With the nightly developer option "Treat calls as system calls" off, no
`CXProvider` is made on iOS (the first system's call makes it) and nothing is registered with Telecom on Android (the call's
foreground service runs, with its own focus and its card, "The call surface" below); the audio session is the app's, activated without
waiting for the system (`call_activates_audio(false)`), and the media controls are the app's (`call_replaces_media_controls(false)`).
The option is read when each call starts; one call keeps the kind it started with. The rest of this section is what the
option turns on.

The room's call is then a phone call of the system's own, so the lock screen, the system's in-call screens and a Bluetooth or car
kit show it and act on it (operator, 2026-11-02, after a device test showed only a Now Playing card). It is the plugin's
call service (`sidevoice-mobile-plugin`'s `call_service`), started and ended as above (`mobile/tauri/src/call.rs`).

- **iOS: CallKit**, in Rust (`plugin/src/platform/call_service/ios.rs`, objc2's CallKit bindings; the plugin has no Swift).
  One `CXProvider`, made by the first system's call: audio only, one call at a time, not in the Phone app's Recents, the mark in one
  ink as its icon (`plugin/assets/call-icon.png`, `npm run icons`). The green button asks a `CXCallController` for a
  `CXStartCallAction` whose handle is the conversation's title (the app's name before one is chosen); the provider reports
  it connecting, and connected when CallKit activates its audio. The red button asks for a `CXEndCallAction`; an end the app
  did not ask for (the in-call screen, the lock screen, a headset, a car kit) hangs up the page's call. A refusal (code
  `call-refused`, CallKit's error in the log) leaves the call the app's own: its own audio session and Now Playing.
- **Android: Core-Telecom** (`androidx.core:core-telecom`), in the call's service (`CallService.kt`): once it is in the
  foreground it adds a self-managed outgoing audio call, named after the conversation, and sets it active. Telecom holds the
  call's audio from then (the mode, the focus, the route), so the service gives up its own focus request (a loss reported
  meanwhile is given back as a gain) and chooses the loudspeaker over the earpiece (as iOS does). The notification stays
  the app's card, without its audio switch (the route is Telecom's then). A disconnect from outside the app ends the
  service as its notification's Hang up does (`hang-up`); Telecom ending the call otherwise is `system`. Telecom refusing
  the call (an emergency call on, the registration failing) leaves the call the app's own: the service's focus.
  The manifest adds `MANAGE_OWN_CALLS` (granted at install), `FOREGROUND_SERVICE_PHONE_CALL` (the service is also of type
  `phoneCall`) and `MODIFY_AUDIO_SETTINGS` (below).
- **Mute.** A mute or unmute from the system's side (CallKit's `CXSetMutedCallAction`, Telecom's `isMuted`) is the voice
  host's native mute of the call ("The call surface" below), and is set on the page's own mute, as the person's (`window.__sidevoiceHost.run({command: "mute", muted, page, since})`; nothing when it is
  so already), carrying the call it is for as the interruptions' commands do. The page's mute goes the other way when it
  changes: as a `CXSetMutedCallAction` the app asks for, and on Android as the system's microphone mute (turned off again
  when the call ends, if the service turned it on), so a car kit shows what the page shows.
- **Hold.** The system holding the call (another call taking its place: `CXSetHeldCallAction`, Telecom's `onSetInactive`)
  pauses it as an interruption does (D-AUDIO-FOCUS, "Audio interruptions" below), and taking it off hold resumes it.
- **The title** follows the conversation: CallKit gets a call update, Android's notification a new name (Telecom's own
  name for the call is the one it started with: Core-Telecom 1.0 has no update for it).
- **The media controls** (docs/MEDIA_CONTROLS.md): on iOS none while the CallKit call lasts (no second card on the lock
  screen; a headset's buttons come through CallKit); on Android the media session stays, without a card, because Telecom
  leaves a self-managed call's headset buttons to the app.
- **App Store.** Apple does not accept CallKit in apps sold in China: the app's availability there is an operator decision
  before a release.

## The call surface

The call's own surfaces, drawn by the app rather than by the system's call (D-NO-CALLKIT-DEFAULT): the iOS Live Activity
(the Lock Screen card and the Dynamic Island) and Android's ongoing call notification. Their design is
research/live-activity-design-2026-11-02 (HANDOVER.md); this is how their data gets there and how they act on the call.

- **One snapshot for both.** The page projects it in its room snapshot (sidevoice-web's `callSurface`, beside the
  `callCard` the report already carries), and the bridge relays it in every `call_state` report as `surface`. The app reads
  it with the room's `mood`, `turn`, `busyReason` and `canSkip` (`mobile/tauri/src/surface.rs`) into the plugin's
  `CallSurface` (`mobile/tauri/plugin/src/call_surface.rs`), which travels in the `CallDetails` the call's service is
  started and updated with, on every change, at most once every 250 ms (Android drops a notification's updates past
  about five a second; a change inside that is shown at its end). Every field has a default: a page that sends less, or
  a surface the app cannot read (logged), shows an empty, idle call, and never stops the call.
- **What it carries** (camelCase, as Android's `updateCallService` receives it in `surface`): `phase` (`idle`,
  `listening`, `transcribing`, `said`, `working`, `speaking`, `holding`, `offline`: the page's state line's order,
  `said` while the person's words just sent show), `canSkip`, `canCancelSend`, `model` and `machine` (worded by the page),
  `workingSince` (ms since the epoch, the room's busy clock, which may start a few seconds before the stage shows
  working; absent when the page does not know it: no time is made up), `still`, up to three `others` (`{title, still}`),
  `conversationCount` (every conversation, the current one included), `page` (`{text, lit}`, the spoken page while
  speaking, `lit` in UTF-16 code units of `text`: the voice's position, which only paces the chunks), `said`, and `words` (`state`, `offline`, `mute`, `unmute`, `skip`,
  `skipName`, `cancelSend`, `hangUp`, `route` with `{route}` to fill, `speaker`, `conversations`), all in the page's
  language. `language` is the page's (`callCard.language`; empty: the device's), for what the system words itself
  on a surface (iOS: the working timer on Always-On). `offline` is always there: a surface the app stopped updating draws the offline state with it. While the call
  switches conversation, the surface keeps what it showed until the new conversation's report comes.
- **The creatures, by reference.** A still is `{id, k, file}` in the snapshot: the page's id (`<threadId>#<px>`; 240 px
  for the current conversation, 108 for the others), `k` (how much of the still the body takes, 1 by default) and `file`,
  the PNG's name in the stills folder, which the app fills (the id's bytes in hex, `.png`). The page hands each PNG over
  once per id (`host.callSurface.still({id, png, px})`, base64, the app's `call_still`) and forgets the ones it shows no
  more (`host.callSurface.forget(ids)`, `call_forget`). The bridge sends the reports, the stills and the forgetting one
  after the other, and a still is answered only once its file is kept, so a report never names a still the app does not
  have yet. Both are done on the call's own thread, in order with the reports: a forgotten still's file stays until
  neither the surface the service last showed nor the next one draws it (an update can wait up to 250 ms). The app keeps
  a still only if it is a PNG of `px` × `px` (its `IHDR`; at most 480 px and 512 KB, an id of at most 120 bytes, at most
  8 MB in all) in the stills folder (`stills_dir`): `call-stills/` in the App Group container `group.ai.sidevoice.mobile`
  on iOS, where the widget extension reads it, or the app's cache where there is no shared container (Android, and iOS
  until the app is entitled to the group). It is emptied at the app's start and when a page loads anew (its call ended
  with it); the new page sends its own.
- **What the person does on a surface** comes back as the service's event `{"type":"action","action":…}` (`mute`,
  `unmute`, `skip`, `cancel-send`, `hang-up`, `open-conversations`; `CallEvent::Action`), from the notification's buttons
  or the Live Activity's intents, or on iOS as the link the app is opened with (`sidevoice://conversations`, Tauri's
  `RunEvent::Opened`). Each call action has one native owner, the one the page's own button ends in too, and the app
  calls it itself (`Room::act`, `mobile/tauri/src/call.rs`): with the phone locked the WebView may not run script, so
  nothing waits for the page. **Mute** (the Live Activity's mute button asks `toggle-mute`: the app takes the other of the mute it knows, never the
  value the card's toggle sends, which a locked phone has sent wrong) is the voice host's mute of the room's call, the lane the microphone is routed to
  (`AppHost::mute_microphone`, the same `VoiceState` mute as the page's `voice_mute`), and the call's surfaces show it at
  once, past the update interval, while the page's reports from before it (one that disagrees) are not taken (the toggle's value and the system's call agree); **cancel send** is its cancel of the unreported turn
  (`AppHost::cancel_microphone_input`, as `voice_cancel_input`); **skip** its cancel of what is said
  (`AppHost::skip_playing`, as `voice_cancel_say`); **hang up** drops what is said, stops every call of the page and
  routes the microphone nowhere (`AppHost::stop_calls`; the audio session is given back once the last call's audio has
  stopped), and ends the call's service and its Live Activity; the page's reports of that call joined (sent before it
  ran the hang-up) do not start it again. The page is told too, first, as its own command for its call (`mute` with the
  value, `skip-reply`, `cancel-input`, `hang-up`, `open-conversations`), and runs it when it runs again: it shows what was
  done and does what only it can (the room told of the hang-up, its lanes disposed of; a stop of a stopped call still
  answers). The stack of the other conversations is the page's alone. The audio route is not one of them: the system's
  route is the native side's to switch. A headset's play/pause and the system's call's mute go through the same native
  mute, and the system's call ending from outside the app hangs up natively too.
- **Android's card** (`CallCard.kt`, `CallCardViews.kt`, `res/layout/call_card*.xml` in the plugin): the call service's
  notification is a `DecoratedCustomViewStyle` (the system draws the header, with no time) whose RemoteViews draw the
  design's Lock Screen card in dp: expanded (the tile, the title, model · machine, the state with its dot and, while
  working, a `Chronometer` since `workingSince`; the audio switch while headphones are connected; mute, skip, the stack
  of the other conversations and hang up; mute is the pill with its label only when the label, measured with the pill's
  font, fits whole in the row, and otherwise the round glyph button: a button never ends in an ellipsis), collapsed (the
  face, "{title} · {state}" where only the title is cut, mute, hang up) and a public version
  for a Lock Screen that hides sensitive content ("Sidevoice", "In call"). The tile and the stack of faces are bitmaps
  drawn by the plugin (`CallCardArt`: the tone's glow, the creature enlarged by its `k` so an accessory stands out, grey
  while offline, the red mic badge), from the stills folder; DM Sans is bundled (`res/font`). The words are the page's;
  the plugin's `res/values*` (the eight languages the app ships) hold only what shows before the page's words come, the
  public version and the channel. The buttons are PendingIntents to the service, so they work on the Lock Screen without
  unlocking: hang up ends the service (`hang-up`), mute (with the value the button shows) and skip are reported as
  actions, the stack opens the app's activity with an extra the plugin turns into `open-conversations`, and the audio
  switch (Android 12 and later, the app's own call) moves the call between the headphones and the loudspeaker with
  `AudioManager.setCommunicationDevice`. The card is drawn again on every update, when a headset comes or goes, and when
  the appearance changes, at most once per 250 ms (what asks inside it is drawn at its end); a still is decoded again
  when its file is written anew under the same name. The own call is kept off the earpiece: a headset, else the
  loudspeaker, also when headphones are removed. The card's channel is `call-card` at DEFAULT importance with no sound,
  vibration or badge (the old LOW `call` channel is deleted). `CallCardRenderTest` draws every state to PNG (light and
  dark, mic on and off, long texts)
  through the platform's own template in Robolectric; `cargo xtask mobile test android` copies them to
  `dist/android-call-card`, and the mobile workflow uploads them.


## The call's Live Activity (iOS)

The iOS face of the call surface: the Lock Screen card and the Dynamic Island (compact, minimal and expanded, variant B),
drawn as research/live-activity-design-2026-11-02 measures them (HANDOVER.md §4–§6, spec/GEOMETRY.md). It exists only
while the app's own call does: the app starts it when its call starts (the app is in front then), updates it with every
change of the call surface, and ends it at once (`.immediate`) when the call ends; one an earlier run left is ended when
the app starts. With the developer option "Treat calls as system calls" on, none is started: the system draws that call
itself (D-NO-CALLKIT-DEFAULT). Nothing runs before iOS 16.1 or while the person has turned Live Activities off for the app;
its buttons need iOS 17 (before that it only shows the call).

- **Where the code is.** ActivityKit has no Objective-C API, so the activity is Swift, in one local package,
  `mobile/tauri/apple/CallActivity`: `CallActivity` (the attributes and state, the views and DM Sans, OFL), linked by the
  app and by the widget extension, and `CallActivityApp` (start, update, end; the app only). The
  Rust side (`mobile/tauri/plugin/src/platform/call_surface/ios.rs`) reaches it by name, `SidevoiceCallActivity`
  (`start:`, `show:`, `end`, each with the call's details as JSON), from the call's service functions
  (`start_call_service` for the app's own call, `update_call_service`, `stop_call_service`).
- **The Xcode project** gets it through the Tauri CLI's own input for it, a custom XcodeGen template
  (`mobile/tauri/ios-project.yml`, `bundle > iOS > template`): the CLI's template plus the package, the app's file
  (`mobile/tauri/apple/App`), the buttons' App Intents (`mobile/tauri/apple/Intents`, compiled into the app and into the
  extension), the App Group `group.ai.sidevoice.mobile` on the app and the
  extension, and the widget extension `SidevoiceActivity` (`ai.sidevoice.mobile.activity`, `mobile/tauri/apple/Widget`),
  embedded in the app, at the app's version. When the CLI's version moves, its template is taken again and these
  additions kept. `Info.ios.plist` declares `NSSupportsLiveActivities` and the app's URL scheme, `sidevoice`
  (`sidevoice://conversations`, the stack of the other conversations; `sidevoice://call`, the rest of the activity).
- **The activity's life.** The system can end the activity (or the person can swipe it away) during a call. The app
  follows its state (`ActivityLifecycle`, `activityStateUpdates`), drops it when it is over and requests a new one with the
  next state (at once if the app is in front, else when it next is); at launch and whenever it comes to the front it ends
  the app's activities that are not the call's (what a crash left). The buttons log each step to the unified log, subsystem `ai.sidevoice`, category `intent`.
- **Updates.** Every change goes out at once. **The words** (the expanded island only: never on the Lock Screen, in the
  compact or minimal) are chunked by the app, where the updates are made (`TextChunker`, `TextPager` in the package; the
  port of research/live-activity-design-2026-11-02/src/text-chunks.js): the page's `page.text` is cut into chunks that fit
  four lines (your words: two lines, 2.5 s each, at most three, the last ending in "…"), measured with DM Sans 550 at 15 pt
  at 330 pt, whole sentences first, then a clause break, then a word; no karaoke. One update per chunk: a chunk stays at
  least 2.5 s and the next shows only once the voice (`page.lit`) has reached its first word, jumping to the chunk the voice
  is in, never ahead of it; the controller wakes itself when the next is due. If a device ever has to cut a chunk it logs its length, never its words (category `text`).
  Your words outlast the page's 3 s (three chunks take up to 7.5 s): while the call is idle, working or can't receive, the app
  keeps them, by conversation, until their last chunk has had its time; listening, speaking, offline or another conversation
  takes over at once. During your words the header says working with its timer only when the page says so (its state line and
  `workingSince`, which need the web release that sends them, sidevoice-web#474, pinned in `web.pin.json`); before that pin it
  wears the page's own line (idle, grey, no timer). Each update moves the activity's

  `staleDate` three minutes ahead, and the activity is updated every minute while the call lasts even when nothing
  changed, so only an activity the app stopped updating goes stale; a stale one draws the offline state with the page's
  offline word (D-LA-OFFLINE). The updates and the end reach ActivityKit one after the other, in order. A call that
  starts while the app is in the background (iOS lets an app request an activity only in front) gets its activity when
  the app next comes to the front, if the call still lasts. A system's call the system refuses goes on as the app's own,
  with its activity.
- **Its buttons** are App Intents (`LiveActivityIntent`, run in the app's process, not offered to Shortcuts), declared in
  the app's and the extension's own code (`mobile/tauri/apple/Intents/CallIntents.swift`, in both targets), as Apple's
  Live Activity intents are, so that each bundle's App Intents metadata lists them as its own. Not in the package: the
  device registers no metadata for intents of a statically linked package (`linkd`: `aggregateMetadataIsEmpty`, and every
  tap fails with "There is no metadata for SetMicOff"; TestFlight build 26), and `cargo xtask mobile check-ios` and the
  archive's check refuse an intent of the package or a package the metadata includes. The extension hands the package's
  views their buttons (`CallControls`); without them (the renders) the views draw the same faces. Mute (a
  toggle with the value), skip, cancel send and hang up ask the Rust side for the page's action through the class it
  defines, `SidevoiceCallSurfaceActions` (`act:`), which reports `CallEvent::Action`. The audio switch, shown only while
  headphones are connected (D-LA-ROUTE), is the system's: it overrides the output to the speaker, or back
  (`AVAudioSession`), and the activity follows the route as it changes; headphones without a microphone keep the switch
  while the call is on the speaker, until they go.
- **The words** are the page's (`words` in the call surface), so the activity has no catalogue of its own; the output's
  name in the audio switch's accessible name is the system's.
- **Diagnostics.** The app's log (the Rust side's `log::`, and the page's console warnings and errors, uncaught errors and
  unhandled rejections through shared/tauri/js/webview-log.js, as on the desktop) is a file, which `log show` does not read; on
  iOS every line also goes to the unified log (`SidevoiceLog`, subsystem `ai.sidevoice`, public strings, category `stills` for
  the creatures' stills, `intent`, `text`, `app`; `app` is private). A still the activity cannot draw is a placeholder, the
  Sidevoice mark (white, 56 % of its slot, on the slot's ground), never an empty slot, and the cause is logged once. A still's
  way is one public `stills` line per step, which `log show --predicate 'subsystem == "ai.sidevoice" AND category == "stills"'`
  reads from a sysdiagnose or `log collect`:
  - the page could not draw it (`console.warn: [sidevoice] call surface: no still for <id> at <px>px: <error>`, sidevoice-web),
    or the app did not answer its `call_still` (`… call surface: call_still <id> failed: <error>`, js/mobile-bridge.js);
  - the app received it (`call_still: <id> (<n> base64 bytes, <px> px) from page <n>`), refused it (`call_still: <id> refused:
    …`, `call stills: refused …`, `call_still: not kept: …`) or kept it (`call stills: kept <path> (<n> bytes, <px> px)`), in
    the folder said once (`call stills: folder <path> (shared container: true|false)`);
  - the activity's state names it (`activity state names stills: <file> (there|<why not>), …`, the app's Swift);
  - the extension drew it (`still drawn: <file>`) or not (`still not drawn: <why>`, `no still named in the call's state`).

  `mobile testflight` prints the entitlements the app and the extension were signed with, and warns when either lacks the
  App Group (`cargo xtask mobile testflight`, `ios::check_ipa`).
- **Evidence.** `cargo xtask mobile renders ios` runs the package's tests on an iPhone Pro Max simulator: every state's
  views drawn to PNG at 3× (`ImageRenderer`) into dist/live-activity/, one sheet per state laid out as the gallery's
  `pieces-<state>.png`, plus the mic off, Always-On, stale, the reply's and your words' chunks, missing stills, the audio on the speaker, no headphones,
  long texts and a Pro Max; it also fails a card or an expanded island taller than 160 pt. CI uploads them as
  `Sidevoice-ios-live-activity-renders`.

### The Live Activity on the Simulator

The renders draw the views; the Simulator shows what the system makes of them: its margins, the island's real shape and
height, the Lock Screen. `cargo xtask mobile live-activity ios` (macOS, Xcode, XcodeGen) builds a harness that is for tests
only (`mobile/tauri/apple/Harness`: an XcodeGen spec rendered into a scratch project; `mobile/tauri/ios-project.yml`
names none of it, so nothing of it is in the app that ships):

- **A host app** whose fixture mode is compiled in Debug only. Given the call's details in `LA_FIXTURE` (the JSON the Rust
  side hands `SidevoiceCallActivity`), it writes the creatures' stills where the app writes them (the App Group's
  `call-stills/`, under the Rust side's file names), then calls the app's own bridge, `CallActivityBridge.start`: the
  controller, the text pager and ActivityKit are the app's.
- **The widget extension**, built from the shipping extension's own sources (`apple/Widget`, `apple/Intents`).
- **XCUITest** (`UITests/`): the fixtures (`Fixtures.swift`: every state, mic off, English for the states with words, long texts in both languages,
  stills missing, 6 and 9 conversations, the gallery's texts) and, per fixture, the Lock Screen card (locked, then woken),
  the compact island (the app in the background) and the expanded island (a long press on it, with the frames in pt of
  what SpringBoard shows at the top of the screen, as JSON: the island's and its elements'). Not the minimal island: a
  second activity of the same app does not make the island show two minimal views.

It runs on the newest iPhone Pro simulator, in light and then in dark (every state; the variants in light only), into `dist/live-activity-sim/`
(`<presentation>-<state>[-<variant>]-<light|dark>.png`, and `INDEX.md`, which names the design gallery's PNG each shot is
to match). `LA_ONLY` (fixture names, comma-separated) shoots only those. CI: `.github/workflows/live-activity-sim.yml`, on
the pull requests that touch the activity's Swift, the Rust that makes its surface or the harness, and by hand with `ref`
(another branch or `refs/pull/N/head`, whose Live Activity it shoots with the harness of the commit it runs on); the
artifact is `live-activity-sim`.

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
  universal release APK (arm64-v8a and armeabi-v7a), as the artifact `Sidevoice-android-unsigned-apk`. The build ends with the 16 KB check Google
  Play makes (`cargo xtask mobile check-16kb [APK]`): the APK's uncompressed libraries start on 16 KB boundaries
  (`zipalign -c -P 16`) and every loadable segment of every library is aligned to 16 KB (the ELF program headers); it
  fails on any other and when there is no library. The job's log lists each library with its alignment.
- **iOS** (macOS, XcodeGen): `cargo xtask mobile generated ios`, then `cargo xtask mobile build ios`, the app for the
  simulator, unsigned, as the artifact `Sidevoice-ios-simulator-app`. When the committed project is not what the CLI
  wrote, the job uploads what it wrote (`mobile-gen-apple`) to commit as `mobile/tauri/gen/apple`.
- **Live Activity renders** (macOS): `cargo xtask mobile renders ios`, every state of the call's Live Activity as PNG, as
  the artifact `Sidevoice-ios-live-activity-renders` (above, "The call's Live Activity").
- **Live Activity on the Simulator** (macOS, `.github/workflows/live-activity-sim.yml`, its own workflow): `cargo xtask
  mobile live-activity ios`, the artifact `live-activity-sim` (above, "The Live Activity on the Simulator").
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

`.github/workflows/mobile-android-signed.yml` signs the Android app with the upload key. It is run
on every push to `main` and by hand (`workflow_dispatch`), and its job refuses any ref but `main`; no pull request runs it. It builds the same unsigned APK
as above, then `cargo xtask mobile sign android` aligns it for 16 KB pages (`zipalign -P 16`), signs it (`apksigner`),
verifies the signature and checks that every native library's loadable segments are 16 KB aligned, which Google Play
requires (`cargo xtask mobile check-16kb [APK]` is that check alone). The result is the artifact
`Sidevoice-android-signed-apk`, and the file `Sidevoice_nightly_android.apk` (with `Sidevoice_nightly_android.apk.sha256`) on the public `nightly` pre-release, which anyone can download: https://github.com/sidevoice/sidevoice-desktop/releases/download/nightly/Sidevoice_nightly_android.apk (RELEASING.md → "Nightly").

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

`.github/workflows/mobile-testflight.yml` signs the iOS app and uploads it to TestFlight. It runs on every push to `main`
(no path filter, like the signed Android build) and by hand (Actions → mobile-testflight → Run workflow, on `main`); never
on a pull request, and every other mobile build stays unsigned with no secret. Uploads never overlap and a running one is
never cancelled (`cancel-in-progress: false`); GitHub keeps only one pending run per concurrency group, so pushes that land
during an upload collapse into the newest one. Its one step is `cargo xtask mobile testflight --build-number <run number>`
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

- **Symbols.** The archive's dSYMs and the app's executable as it ships are kept as the run's artifact
  `Sidevoice-ios-<N>-symbols` for 90 days, and the upload carries the symbols for App Store Connect (`Symbols/`, checked
  in step 2), so TestFlight's crash reports are symbolicated. The Rust code's names are in them: the Rust library is
  linked as a static library, which the release profile's `strip` does not touch, and only the archive's last step
  strips the executable, after Xcode has written the dSYM. A crash report from the phone (`.ips`) gives offsets into
  the executable (`imageOffset`): `atos -o Sidevoice.app.dSYM/Contents/Resources/DWARF/Sidevoice -l 0x100000000 <0x100000000 + offset>`,
  or `llvm-symbolizer`. A build made before they were kept has its symbols rebuilt by mobile.yml, run by hand with
  `symbols-of-build: <N>` on a ref whose app is that build's commit (`cargo xtask mobile symbols`: the same archive,
  unsigned and not uploaded, so its code is laid out the same). **A rebuild is not proof**: Xcode is not pinned on the
  runner, so it may not produce the shipped binary, and symbols of another binary read as plausible, wrong names. The
  job prints `dwarfdump --uuid` of the rebuilt executable and dSYM: compare it with the `.ips` header's `slice_uuid`
  (dashes and case aside) before trusting anything. With the report at hand,
  `cargo xtask mobile symbols --build-number <N> --ips <file>` does it and fails on a mismatch. The rebuild needs a branch
  of the build's commit plus this change (`mobile symbols` and the `symbols-of-build` input). "Symbolicated" means
  function names only: the release profile has no debug info, so no file:line (that would take
  `CARGO_PROFILE_RELEASE_DEBUG=line-tables-only` in the TestFlight job; not set).

- **The build number** is the run number of the workflow, which only grows, as TestFlight requires of the builds of a
  version, and it is the app's `CFBundleVersion` whole. (The CLI's own `--build-number` appends it to the version,
  `0.1.0.7`: four numbers where Apple accepts three.) The version, `CFBundleShortVersionString`, is `tauri.conf.json`'s.
  Every push to main is a new run, so a new number. A re-run of a run whose upload App Store Connect accepted has the same
  number and fails: dispatch again (a new run, a new number) rather than re-running.
- **Credentials.** The repository secrets `APPLE_API_KEY` (the key's id), `APPLE_API_ISSUER`, `APPLE_API_KEY_P8` (the
  .p8's contents) and `APPLE_TEAM_ID`, the same the desktop notarization uses; the key needs the **Admin** role in App
  Store Connect (the Developer role cannot create the certificate or the profile). Only the step that signs has them;
  the .p8 is written to a file only the job's user can read, in a directory of its own, and removed when the command ends;
  the contents are taken out of the command's environment as they are read, so no process it starts (the Tauri CLI,
  `xcodebuild`, `altool`) gets them, only the file's path and the key's id and issuer. The checkout keeps no token.
- **Main only.** The job refuses any other ref (`if`), so does the command, and the job runs in the environment
  `testflight`. The gate that a workflow file on another branch cannot lift is the environment's: its deployment
  branches are `main` only (Settings → Environments), and the key's secrets can be moved into it.
- **Before the first upload**, in App Store Connect and the repository, once:
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

The release build kind (D-MOBILE-CHANNELS): every build shipped today is a nightly, and the release channel comes later.
