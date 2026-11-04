# Other apps' media during a call

Operator decisions (2026-11-04, sidevoice-desktop#187). While the person's microphone is open in a call (joined, and not
muted) **or the agent is speaking in it** (the microphone muted or not), other apps' media is lowered (the default) or
paused; when neither holds any more, the app gives back **only what it took**. The choice is `host.mediaDuck`
(docs/BRIDGE.md), kept in `settings.json` as `mediaDuck`: `lower`, `pause` or `off`.

"The microphone open" means the person's microphone open **in a call** (`micEnabled` of the joined call), nothing else. A
microphone that listens all the time for a wake word (a possible future feature) must not count: it would keep other apps'
media lowered or paused all day. Such a feature reports its listening apart from `micEnabled`, and `wanted` does not read it.

## When

`wanted(mode, joined, mic_enabled, agent_speaking)` in core: the mode while `joined && (mic_enabled || agent_speaking)`,
nothing otherwise or with `off`. "Agent speaking" is the room's own report (`bridge_state`, the snapshot's
`agent == speaking`, which the page sets while the agent's voice plays, `botLive`). Going to a mode is followed at once;
going back to nothing is **held for 1.2 s** (`HOLD_MS`, core `Hold`), so the gap between the person's turn closing and
the agent's reply starting does not make the music come up and go down again. Hanging up, quitting or choosing `off`
gives back at once.

## Where it lives

| What | Where |
|---|---|
| The rule: when, the hold, what to take, what to give back | `desktop/core/src/media_duck.rs` (pure, unit-tested with a fake system) |
| The worker thread that follows the call and the choice, and the `media_duck` command | `desktop/tauri/src/media_duck.rs` |
| What each OS can observe and do | `desktop/tauri/src/platform/media_duck/{windows,linux,macos}.rs`; macOS taps in `macos_taps.rs` |

The call reaches the worker from `bridge_state` (the page's report: `joined`, `micEnabled`, `agent`) and from the reset of
the call; quitting releases first. The worker is one thread because every system call may block; it wakes for the end of a
hold on its own.

## Giving back only what was taken

`Ducker` remembers each player it lowered (the volume it had, and the volume the system read back after the app set it) or
paused. On close it restores a volume only if the player is still at that volume (the person changed it: left alone),
resumes a player only if it is still paused (the person pressed play, or stopped it: left alone), forgets one that has
gone, and never starts what was not playing. A step the system refused is not remembered; giving back that fails is tried
again at the next change of the call, three times. Each player is found again by its key on the device it was taken from
(not the default device's now). What is owed is written to `media-duck.json` in the app's data folder after every change
and given back when the app starts, so a crash or a force quit does not leave other apps lowered or paused. The file is written
beside itself and renamed over it. It carries the machine's start time and the time written: a pause owed from another start
of the machine, or older than an hour (each pause has its own time; one held by a long call is given back when the microphone closes, one only left over is not), is dropped (the player is not where it was, and a person who left it paused must stay
so); a lowered volume is kept, since the system stores it, but after a restart its stream, session or device id means nothing: it is found by its app alone, and dropped when it has none. Debts are settled newest first.

The system remembers a volume per app (the Windows mixer, PipeWire's and PulseAudio's stream restore, Music's and Spotify's
own volume), so a stream ending does not end the debt: the player is looked for again by its key and, failing that, as the
app's next one (Windows: the session identifier; Linux: the application name, else the binary; macOS: the app's name), and
given back its volume if it starts at the one the app set. An app that is not playing keeps its debt (at most 32 are kept).
A player that cannot be asked now (a failure that may pass) keeps its debt too. Retries are counted per change of the call,
not per report of the page.

Pause only pauses: a player that cannot be paused is left as it is, never lowered instead (core `take`; operator,
2026-11-04).

The app's own sound is never touched: the process table is read and every process that descends from the app (its
webview's helpers) is skipped for audio sessions and streams, and MPRIS players are skipped by the process behind their bus
name. On Windows a media session whose app id names this executable or the webview is skipped. On macOS a process is also
skipped when this app is responsible for it (`responsibility_get_pid_responsible_for_pid`): the webview's
`com.apple.WebKit.GPU` is an XPC service whose parent is launchd, so its ancestry does not lead here.

## Per OS

| OS | Lower | Pause |
|---|---|---|
| Windows | Each other process's audio session on the default output device (WASAPI `ISimpleAudioVolume`), found again by endpoint id and session instance id. Per app. | System media transport controls (`GlobalSystemMediaTransportControlsSessionManager`): any app on the media card; one session at a time, told apart by its place among its app’s sessions (`media:<app id>#<n>`; a session that appears or goes in between can shift it). |
| Linux | Each playing stream's volume through the sound server (`pactl`, PulseAudio or PipeWire's Pulse server), so browsers are reached. Needs a `pactl` that lists as JSON (16 or later), run with `LC_ALL=C` and a 2 s limit; otherwise the page is told `lower: none`. Per app. | MPRIS `Pause`/`Play` of each player that is playing and `CanPause`. |
| macOS 14.4+, System Audio Recording allowed | **Any app that plays**, one Core Audio process tap each (below), browsers included. The output volume and the app's own voice are never touched. `lower: per-app`. | **Music and Spotify only**, through Apple events (operator, 2026-11-04): YouTube, browsers and other apps are left playing, never lowered instead; the page says so. |
| macOS 13 to 14.3, or the permission refused / not asked | Music's and Spotify's own `sound volume` through Apple events (never the device volume); nothing else. `lower: some-apps`. | Music and Spotify only. |
| iOS, Android | Not in this change (see the PR). | Not in this change. |
| Browser | Not possible; the page has no `host.mediaDuck` and shows nothing. | |

Apple events (Music, Spotify) need Automation, which the system asks once per app, and only for an app that is open; it is
asked when the person presses "Allow" on its row in the list of permissions (`allow("automation")`), never at startup, on
choosing a mode, or by a call. No media key is ever sent: it
toggles, and during a call it reaches this app (the Now Playing one).

Limits that apply to all: it looks at what plays when the mode is taken, not after (a player that starts later is not
touched); a player that does not report itself to the system's media controls (a game, a call in another app) is not
reached by pausing, and lowering only reaches what the OS lists.

## macOS process taps

Research and sources: `briefs/night-2026-11-03/research-187-macos-ducking.md` (option C). Used from **macOS 14.4**: the tap
functions exist from 14.2, but System Audio Recording, `NSAudioCaptureUsageDescription` and the prior art followed here
(AudioCap, FineTune) start at 14.4; 14.2 and 14.3 take the Apple events path. The tap functions are looked up at run time
(`dlsym`), never linked, so the app still starts on macOS 13.

1. **What plays**: the audio system's process objects (`kAudioHardwarePropertyProcessObjectList`) with output running
   (`kAudioProcessPropertyIsRunningOutput`), without this app's own (its pid, what it is responsible for, what descends
   from it, what shares its responsible process, a WebKit helper whose responsible process cannot be told) and system
   sounds (alerts, Siri, dictation, speech, accessibility, by bundle id). A process that plays only on another output
   (`kAudioProcessPropertyDevices`: AirPlay, HDMI, an audio interface) is left alone. Listing needs no permission. Taken
   when the mode is taken, as everywhere.
2. **Tap**: one per process, `CATapDescription` stereo mixdown of it, private, `mutedWhenTapped` (the app is silent only
   while this app reads the tap, so if this app stops or dies the app's own sound comes back), named `Sidevoice-duck-<this
   app's pid>`, limited to what the process sends to the default output (`deviceUID`): what it plays elsewhere stays there.
3. **Private aggregate device** on the default output (an aggregate output, a Multi-Output Device, is flattened to its
   active hardware sub-devices, its main one the clock, the others with drift compensation), the tap in its tap list, tap
   auto-start on, **tap drift compensation off on Bluetooth** (it crackles there), on otherwise; the hardware's own input
   streams turned off for the IOProc (`kAudioDevicePropertyIOProcStreamUsage`), so a headset's microphone is not opened by
   it. If they cannot be turned off, the tap is not made.
4. **IOProc**: the tap's samples to the output at the gain (`LOWERED_TO`, 0.25 of the volume, as on the other OSes), ramped
   over 30 ms so the dip and the restore do not click; atomics only, no lock, allocation or log.
5. **Giving back** is destroying the tap: ramp to 1, then stop, destroy the IOProc, the aggregate and the tap, in that
   order. The app's own volume is never touched, so nothing of a tap outlives the process (its debt has no app).
6. **Output changes**: listeners on the default output device and on the output's rate and life; after 250 ms of quiet
   (a Bluetooth headset switching to its call profile changes rate when the microphone opens) every tap is made again on
   the output as it is now, the old one silenced first and the new one ramping up from silence; one that cannot be made
   again is given back. With no output, or System Audio Recording no longer allowed, every tap is given back instead (a
   tap that mutes without the permission would silence its app). Taps are started outside the lock that guards them.
   A tap that starts delivering only zeros (Apple forum 825780) is not detected yet.
7. **Leftovers**: at start, before anything, aggregates and taps named `Sidevoice-duck-` are destroyed (private ones
   should go with their process; other tap apps clean anyway).
8. **Permission**: System Audio Recording (TCC `kTCCServiceAudioCapture`). Its state is read with `TCCAccessPreflight`
   (private, not entitlement-gated, what AudioCap uses), `capture` in `get`. It is asked only when the person presses
   "Allow" on its row in the list of permissions (`allow("capture")`: the setup's permissions step, Settings → Permissions),
   never on choosing a mode or in a call: a tap that mutes nothing is read until the person answers (60 s at most), which is
   when the system asks; one question at a time. **No tap that mutes is ever made without it** (the app would go silent
   instead of lower). `openSettings("capture")` opens Privacy & Security → Screen & System Audio Recording,
   `openSettings("automation")` Privacy & Security → Automation.

Usage strings (`NSAudioCaptureUsageDescription`, `NSAppleEventsUsageDescription`) are English only for now;
per-language `InfoPlist.strings`, as the mobile app has, are a follow-up (with the Spanish `NSMicrophoneUsageDescription`).

## What `get` answers

`{mode, lower, pause, access, capture?}`:

- `lower`: `per-app` (any app, one by one), `some-apps` (only named apps: macOS without taps) or `none`;
- `pause`: `media-controls`, `some-apps` or `none`;
- `access`: macOS Automation of Music and Spotify, `granted`, `denied`, `undetermined` or `unknown` (the system answers only
  for an app that is open: with neither open the last real answer stands, else `unknown`); absent elsewhere;
- `capture`: macOS 14.4 and later only, System Audio Recording, `granted`, `denied`, `undetermined` or `unknown` (the
  check could not be made); absent elsewhere.

How far this system can carry out the choice, for the page to say so. `access` and `capture` are the permissions the page
lists (Settings → Permissions and the setup's permissions step, each a row with Allow or Open System Settings); the Call
row only says one is missing and links there. A mode the system cannot do is still kept; it does
nothing.

## Device test checklist

The per-OS checklist is in the pull request (#408). macOS items to verify on a device, from the research:

- No prompt for listing processes; the System Audio Recording prompt appears only on its "Allow" in the list of permissions.
- Denied: nothing goes silent (no muting tap is made); Music and Spotify are lowered through Apple events instead.
- `kill -9` while lowered on 14.x, 15.x and 26: the other apps come back at once (`mutedWhenTapped`), and the next start
  finds nothing named `Sidevoice-duck-` left.
- AirPods: the call profile switch when the microphone opens: a short dip, then the app lowered, no crackle (drift off).
- Device switch while lowered (headphones in/out, USB DAC, HDMI): lowered on the new output, nothing silent.
- Browsers: Chrome/YouTube, Safari/YouTube (WebKit GPU process) lowered; Sidevoice's own voice and page sounds untouched.
- The hardware input streams are the ones turned off (the tap's stream comes after them in the aggregate).
- The Settings anchor `Privacy_ScreenCapture` opens the right pane on 14.x and 15.x.
- Another tap app (SoundSource, FineTune) running on the same device.
- A tap that starts delivering zeros (Apple forum 825780).
