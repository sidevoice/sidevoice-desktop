# Microphone access

`platform/microphone.rs` owns `MicrophoneAccess::{state, request, open_settings}` and `AccessState`:
`granted`, `denied`, `not-determined`, `no-device`, `unavailable`. Denial takes precedence over absence.
The platform reads again on every operation; opening settings does not change or establish permission.

- **macOS:** the original AVFoundation authorization/device check and one-shot request callback, moved into
  `platform/microphone/macos.rs`. The existing `host.microphone` methods, wire values and TCC behavior are unchanged.
- **Windows:** the existing maintained `winreg` dependency reads the machine microphone switch and the user's
  `ConsentStore\\microphone\\NonPackaged` desktop-app switch. No registry writes, shell, elevation, or new dependency.
  We use the registry option because this is an unpackaged NSIS app, whereas Microsoft's
  [AppCapability preflight guidance](https://learn.microsoft.com/en-us/windows/apps/develop/camera/camera-privacy-setting)
  targets packaged apps. The [desktop-app privacy switch](https://support.microsoft.com/en-us/windows/privacy/windows-camera-microphone-and-privacy)
  is distinct from the Store-app switch; per-executable usage timestamps are not consent.
  A registry allow alone never establishes working capture. Missing/unreadable/unrecognized keys leave a browser
  permission hint unavailable; only an actual successful capture can establish access in that case. WebView2 supplies device presence and capture failures.
  The existing `ms-settings:privacy-microphone` opener plugin path and room/origin checks remain.
- **Linux:** capture currently belongs to WebKit, which uses the installed session audio backend. Native voice is
  still macOS-only on this base. We consume that existing backend's getUserMedia observations, without a second
  PipeWire/Pulse client, a native audio probe, a portal, or an invented system permission prompt/settings URI.
  `NotAllowedError`/`PermissionDeniedError`/`SecurityError` mean denied; `NotFoundError`/`DevicesNotFoundError` mean
  no device. `NotReadableError`, `TrackStartError`, `AbortError`, constraint failures and unknown errors mean
  unavailable, **not** no device or denied. An enumeration failure is not an empty list.

## Additive bridge seam

On page-capture hosts with `navigator.mediaDevices.getUserMedia`, `host.microphone.access` adds:

```ts
interface MicrophoneAccess {
  state(): Promise<"granted" | "denied" | "not-determined" | "no-device" | "unavailable">;
  request(): Promise<"granted" | "denied" | "not-determined" | "no-device" | "unavailable">;
}
```

`state()` queries the current webview permission and input-device list without opening capture. `request()` opens
getUserMedia in the caller's gesture and stops all tracks immediately, then passes the outcome to platform policy.
It does not record audio or keep streams. The fixed settings operation stays at `host.microphone.openSettings`
on Windows; Linux has no `openSettings`. The room-only `microphone` command accepts `access-state`/`access-request`
and a fresh `backend` observation; these observations are not security grants and cannot change the webview's
capture policy. Legacy macOS `state`/`request`/`open-settings` actions retain their response vocabulary.

The current web UI still feature-detects top-level `state` and `request`. Windows/Linux do **not** acquire those
methods, so web#363's page permission fallback and desktop#197's Windows settings button stay intact. The richer
`access` seam is available for the later UI/native-voice integration. It does not poll automatically, cache grants,
replace browser methods or alter the current call path. No UI copy changed.

## Verification and remaining runtime checks

Automated interface tests cover denial precedence, no-device observations, permission changes, unknown observations,
and backend error classification. Bridge tests cover fresh permission/device reads, error forwarding, capture cleanup,
live-track checks, the existing page fallback and Windows settings command. These are deterministic tests with
backend fixtures, **not** proof of real OS permissions or hardware behavior.

| OS | Real device/denial/change paths |
| --- | --- |
| macOS | Unverified in this change: bundled-app TCC first prompt, deny/re-enable, unplug/replug. Existing implementation preserved. |
| Windows | Unverified: actual NSIS app with machine/desktop privacy switches, managed policy, no input device, unplug/replug, returning from Settings. ConsentStore is an OS implementation detail; fresh capture outcome remains necessary. |
| Linux | Unverified: actual WebKit capture over the installed PipeWire/Pulse backend, denied session access, backend outage, no device and unplug/replug. No direct portal permission claim. |

Normal PR CI compiles platform implementations and runs the existing checks; no extra macOS/scratch CI is requested.
A green compilation or fixture test does not verify any row above. To exercise Windows/Linux manually, call
`await window.__sidevoiceDesktop.host.microphone.access.state()` and `.request()` in the room's developer console,
then repeat after changing privacy/device state. `request()` must be invoked from a user gesture if the webview
requires one. Confirm every capture track stops and the hardware capture indicator clears.

## Decision for the operator

The existing web permission contract cannot represent `unavailable`. Recommendation: keep its current fallback,
as this change does, until a coordinated web change can consume all five states and distinguish backend failure
from user denial. Alternative: switch the page to native `state`/`request` now, requiring a web contract/UI change.
No such switch or UI copy change is included in P22.
