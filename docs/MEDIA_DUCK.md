# Other apps' media while the microphone is open

Operator decision (2026-11-04, sidevoice-desktop#187). When the person's microphone opens in a call (joined, and not
muted), other apps' media is lowered (the default) or paused; when it closes (muted, hung up, quit) the app gives back
**only what it took**. The agent's speech does not trigger it. The choice is `host.mediaDuck` (docs/BRIDGE.md), kept in
`settings.json` as `mediaDuck`: `lower`, `pause` or `off`.

## Where it lives

| What | Where |
|---|---|
| The rule: what to take, what to give back, the mode | `desktop/core/src/media_duck.rs` (pure, unit-tested with a fake system) |
| The worker thread that follows the call and the choice, and the `media_duck` command | `desktop/tauri/src/media_duck.rs` |
| What each OS can observe and do | `desktop/tauri/src/platform/media_duck/{windows,linux,macos}.rs` |

The call reaches the worker from `bridge_state` (the page's report: `joined`, `micEnabled`) and from the reset of the
call; quitting releases first. The worker is one thread because every system call may block.

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
so); a lowered volume is kept, since the system stores it, but after a restart its stream, session or device id means nothing: it is found by its app alone (on macOS, the default output device), and dropped when it has none. Debts are settled newest first.

The system remembers a volume per app (the Windows mixer, PipeWire's and PulseAudio's stream restore), so a stream ending
does not end the debt: the player is looked for again by its key and, failing that, as the app's next one (Windows: the
session identifier; Linux: the application name, else the binary), and given back its volume if it starts at the one the app set.
An app that is not playing keeps its debt (at most 32 are kept). A player that cannot be asked now (a failure that may pass)
keeps its debt too. Retries are counted per change of the call, not per report of the page.

The app's own sound is never touched: the process table is read and every process that descends from the app (its
webview's helpers) is skipped for audio sessions and streams, and MPRIS players are skipped by the process behind their bus
name. On Windows a media session whose app id names this executable or the webview is skipped.

## Per OS

| OS | Lower | Pause |
|---|---|---|
| Windows | Each other process's audio session on the default output device (WASAPI `ISimpleAudioVolume`), found again by endpoint id and session instance id. Per app. | System media transport controls (`GlobalSystemMediaTransportControlsSessionManager`): any app on the media card; one session at a time, told apart by its place among its app’s sessions (`media:<app id>#<n>`; a session that appears or goes in between can shift it). |
| Linux | Each playing stream's volume through the sound server (`pactl`, PulseAudio or PipeWire's Pulse server), so browsers are reached. Needs a `pactl` that lists as JSON (16 or later), run with `LC_ALL=C` and a 2 s limit; otherwise the page is told `lower: none`. Per app. | MPRIS `Pause`/`Play` of each player that is playing and `CanPause`. |
| macOS | The default output device's volume (`VirtualMainVolume`), **only while Music or Spotify is playing** (asked through Apple events without opening them); for anything else playing nothing is lowered. **The app's own voice is lowered with the rest.** A device with no volume control (some HDMI outputs) is not lowered. | Music and Spotify only, through Apple events; the person allows each once (Automation). Lower needs it too (to see that they play). It is asked only when the person presses "Allow" in Settings (`allow`), never at startup or in a call; `get` reports it (`access`). No media key is ever sent. |
| iOS, Android | Not in this change (see the PR). | Not in this change. |
| Browser | Not possible; the page has no `host.mediaDuck` and shows nothing. | |

Limits that apply to all: it looks at what plays when the microphone opens, not after (a player that starts later is not
touched); a player that does not report itself to the system's media controls (a game, a call in another app) is not
reached by pausing, and lowering only reaches what the OS lists.

## What `get` answers

`{mode, lower, pause, access}`, where `access` is `granted`, `denied`, `undetermined` or `unknown` (macOS Automation for Music and Spotify, which the system answers only for an app that is open: with neither open the last real answer stands, else `unknown`; `granted` elsewhere), `lower` is `per-app`, `system` or `none`, and `pause` is `media-controls`, `some-apps` or
`none`: how far this system can carry out the choice, for the page to say so. A mode the system cannot do is still kept;
it does nothing.

## Device test checklist

See the pull request.
