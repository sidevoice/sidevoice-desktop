# Call-card capability inventory

P18: `platform/card_window` owns construction, native panel behavior, visibility, destruction and activation diagnostics.
Layout, call state and bridge contracts remain in `call_controls` and the portable core. Geometry, visibility
observations and pointer subscriptions are extracted separately in P19a–P19c. This is an inventory of the existing
card, including its deliberate fallbacks; it does not enable local voice on additional operating systems.

## Evidence and status

`available` requires a test or CI run that exercises the stated behavior. `unverified` means implemented/requested,
but no relevant runtime evidence has been established for this revision. Every unverified cell below means **pending
native runtime validation**; compilation alone never promotes it. `unavailable` includes its specific fallback or limit.
The existing macOS card probe exercises fullscreen nonactivation, hover/cursor, outside clicks, drag, mute and hang-up;
its new PR run must pass before those behaviors can be claimed for this refactor. Ordinary Spaces switching and mixed
DPI / monitor removal still need a real desktop test. No Windows/X11/Wayland runtime parity is inferred from builds.

| Capability | macOS | Windows | Linux X11 | Linux Wayland |
|---|---|---|---|---|
| Hidden creation, transparent borderless card, no window shadow | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test |
| Always above ordinary apps | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unavailable — compositor controls stacking |
| Over full-screen apps | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unavailable — compositor controls stacking |
| Across workspaces / Spaces | unverified — pending runtime test | unavailable — Tauri does not implement Windows all-workspaces | unverified — pending runtime test | unavailable — compositor controls workspaces |
| Show, hover, click and drag without activation / keyboard theft | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test |
| No taskbar button / app-switcher cycling | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test |
| Mouse input, inactive hover and clickable-button cursor | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test |
| Background hand cursor refresh and arrow restoration on leaving | unverified — pending runtime test | unavailable — only page cursor supplied | unavailable — only page cursor supplied | unavailable — only page cursor supplied |
| Card webview stays live while hidden | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test |
| Bundled page only, navigation restriction, all capture permissions denied | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test |
| Show during call when stage absent; enabled / always-show settings | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test |
| Hide by card or tray; reset hiding on stage transition / next call | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test |
| Settings preview, extended during pointer use / drag / resize | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test |
| Call snapshot, mute, hang-up, conversations and device selection relay | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test |
| Room / Settings opening, dismiss, level / playback and shortcut updates | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test |
| Page stage visibility and rectangle reporting | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test |
| Room shown / minimized observation | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test |
| Room workspace / occlusion observation | unverified — pending runtime test | unverified — pending runtime test | unavailable — X11 workspace/coverage not observed | unavailable — focus heuristic only |
| Fraction of stage uncovered by other apps | unverified — pending runtime test | unavailable — coverage not measured | unavailable — coverage not measured | unavailable — other windows not observable |
| Global desktop pointer and card frame | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unavailable — global coordinates unavailable |
| Pointer polling and pointerInside patches | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unavailable — page pointer events instead |
| Outside clicks dismiss card panels | unverified — pending runtime test | unverified — pending runtime test | unavailable — page leave timeout instead | unavailable — page leave timeout instead |
| Global Escape dismissal | unavailable — non-key card does not listen to keyboard | unavailable — non-key card does not listen to keyboard | unavailable — non-key card does not listen to keyboard | unavailable — non-key card does not listen to keyboard |
| Display work areas, mixed DPI and changed-monitor fallback | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unavailable — global placement unavailable |
| Anchored layout, expanding content fitting and corner selection | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unavailable — compositor owns placement |
| Pointer drag, edge magnet and per-display saved placement | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unavailable — compositor drag only; no placement saved |
| Compositor / native window drag fallback | unavailable — own pointer drag used | unavailable — own pointer drag used | unavailable — own pointer drag used | unverified — pending runtime test |
| Page-driven logical window sizing | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test |
| Picture-in-picture grip resize, minimum size and saved per-display size | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unavailable — resize command not offered |
| Reset stored size and placement; bridge place / size patches | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unavailable — no stored desktop placement |
| Latest show / hide and move ordering; no state lock across native queries | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test |
| Window / webview destruction on quit | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test |
| Pointer subscription and watcher shutdown | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test | unverified — pending runtime test |

## Implementation and validation

Tauri's maintained window builder supplies the common path; existing `tauri-nspanel` supplies macOS. Existing
AppKit overrides are retained because the panel's late style-mask change otherwise activates the app and WebKit
otherwise suppresses inactive hover. This refactor adds no new private APIs. See
[Tauri window APIs](https://docs.rs/tauri/latest/tauri/webview/struct.WebviewWindowBuilder.html).

Validation must separate portable core / bridge tests from compositor behavior. Normal PR CI includes the existing
macOS input probe and Windows/Linux builds. No extra private-repository macOS jobs are triggered. Review this matrix
with each interface PR; later evidence updates belong here and in the PR body.
