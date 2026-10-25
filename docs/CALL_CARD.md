# Call-card capability inventory

P18: `platform/card_window` owns construction, native panel behavior, visibility, destruction and activation diagnostics.
Layout, call state and bridge contracts remain in `call_controls` and the portable core. Geometry, visibility
observations and pointer subscriptions are extracted separately in P19a–P19c. This is an inventory of the existing
card, including its deliberate fallbacks; it does not enable local voice on additional operating systems.

## Capabilities

This matrix describes the implemented behavior across main and the #207–#210 extraction stack, including platform
fallbacks. macOS is used daily by the operator; its known hand-cursor glitch is recorded below. X11 window-manager
hints and Wayland compositor policies can limit native window behavior.

| Capability | macOS | Windows | Linux X11 | Linux Wayland |
|---|---|---|---|---|
| Window: always on top | Yes | Yes | Partial — window-manager stacking hint | No — compositor controls stacking |
| Window: over full-screen apps | Yes | Partial — topmost window; exclusive full-screen can cover it | Partial — window manager decides over full-screen | No — compositor controls stacking |
| Window: all desktops / Spaces | Yes | No — no Windows all-desktops implementation | Partial — sticky-workspace hint | No — compositor controls workspaces |
| Window: no focus stealing | Yes | Yes | Yes | Partial — nonfocusable request; compositor controls activation |
| Window: hidden from taskbar | Yes | Yes | Partial — skip-taskbar hint | Partial — skip-taskbar request; compositor decides |
| Window: hidden from Alt+Tab / app switcher | Yes | Yes | Partial — skip-taskbar hint; switcher policy varies | No — no compositor app-switcher exclusion |
| Interaction: hover and click | Yes | Yes | Yes | Yes |
| Interaction: drag | Yes | Yes | Yes | Partial — compositor drag only |
| Interaction: picture-in-picture resize grip | Yes | Yes | Yes | No — resize command not offered |
| Interaction: hand cursor | Partial — not always, depends on what is underneath | Yes | Yes | Yes |
| Interaction: outside click closes panels | Partial — other-app clicks only; own room clicks not observed | Yes | No — page leave timeout instead | No — page leave timeout instead |
| Interaction: global Escape closes panels | No — non-key card has no keyboard listener | No — non-key card has no keyboard listener | No — non-key card has no keyboard listener | No — non-key card has no keyboard listener |
| Placement: corner selection | Yes | Yes | Yes | No — compositor owns placement |
| Placement: edge magnet | Yes | Yes | Yes | No — no global coordinates |
| Placement: remembers position and size per display | Yes | Yes | Yes | No — no saved desktop placement or grip size |
| Placement: mixed DPI | Yes | Yes | Yes | Partial — logical sizing only; no global placement |
| Awareness: room minimized | Yes | Yes | Yes | Yes |
| Awareness: room covered by other windows | Yes | No — detects other virtual desktops, not coverage | No — coverage not observed | No — focus heuristic instead |
| Content: three formats and three preset sizes | Yes | Yes | Yes | Yes |
| Content: captions / transcripts / balloons | Yes | Yes | Yes | Yes |
| Content: mute / hang-up / skip-reply / cancel-input | Yes | Yes | Yes | Yes |
| Content: avatar animation | Yes | Yes | Yes | Yes |
| Content: theme and language synchronization | Yes | Yes | Yes | Yes |

## Not tested yet on real machines

Windows, Linux X11 and Linux Wayland have the implementations and fallbacks described above, but have not yet
been verified on real machines for this stack. The cells describe code capabilities, not runtime test results.

The web-owned content comes from sidevoice-web v0.10.1: `apps/web/src/state/floating-card.ts`,
`apps/web/src/call-controls/CallCard.tsx` and `apps/web/src/call-controls/main.tsx`.

## Implementation and validation

Tauri's maintained window builder supplies the common path; existing `tauri-nspanel` supplies macOS. Existing
AppKit overrides are retained because the panel's late style-mask change otherwise activates the app and WebKit
otherwise suppresses inactive hover. This refactor adds no new private APIs. See
[Tauri window APIs](https://docs.rs/tauri/latest/tauri/webview/struct.WebviewWindowBuilder.html).

Validation must separate portable core / bridge tests from compositor behavior. Normal PR CI includes the existing
macOS input probe and Windows/Linux builds. No extra private-repository macOS jobs are triggered. Review this matrix
with each interface PR when behavior changes.

Windows app-switcher correction: the hidden card's native extended style adds `WS_EX_TOOLWINDOW`, clears
`WS_EX_APPWINDOW` and preserves `WS_EX_NOACTIVATE`. The implementation checks native API failures and reads the
policy back after a nonactivating frame refresh; construction fails if the policy was not retained. Tauri's
`skip_taskbar` does not provide this Alt+Tab policy, so this uses the existing `windows-sys` dependency and the
[documented Windows styles](https://learn.microsoft.com/en-us/windows/win32/winmsg/extended-window-styles).
Revisions before this fix lack Windows app-switcher exclusion.

Tao regenerates extended styles during visibility changes. A card-local `WM_STYLECHANGING` subclass preserves the
policy across those rewrites and removes itself on `WM_NCDESTROY`; it retains no heap state. A Windows-only native
regression test creates a hidden system-class window, applies the policy, simulates a later style rewrite, and
checks exclusion, nonactivation and preservation of an unrelated style. This exception to portable interface tests
is necessary because a Tauri mock cannot exercise Win32 style messages. Cross-target Clippy compiles this test;
execution requires Windows. Normal Windows first-open CI also runs the construction-time policy readback.

On Wayland, the settings preview can expire during hover or compositor dragging: page pointer events do not extend
the native preview lifetime. Other settings, call actions and panel timeouts use the shared page implementation.
