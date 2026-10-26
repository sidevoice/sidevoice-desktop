# Call-card capability inventory

P18: `platform/card_window` owns construction, native panel behavior, visibility, destruction and activation diagnostics.
Layout, call state and bridge contracts remain in `call_controls` and the portable core. Geometry, visibility
observations and pointer subscriptions are extracted separately in P19a–P19c. This is an inventory of the existing
card, including its deliberate fallbacks; it does not enable local voice on additional operating systems.

## Platform interface

This table describes main's existing card behavior as exposed by the completed #207–#210 stack, not runtime test
status. `native:` means an OS operation, including one reached through a shared Tauri default; `fallback:` spells
out a no-op, constant or unavailable observation. Linux cells split X11/Wayland only where behavior differs.
All 13 operations are listed (trait methods, and `desktop_geometry`'s plain functions); cursor maintenance is a separate private operation invoked by `watch`.

| Method | What it does | macOS | Windows | Linux (X11 / Wayland) |
|---|---|---|---|---|
| `CardWindow::configure` | Install window support in the app builder. | native: installs `tauri_nspanel` | fallback: returns builder unchanged | fallback: returns builder unchanged |
| `CardWindow::create` | Construct the hidden, nonactivating card. | native: status-level NSPanel, all Spaces/full-screen auxiliary, nonactivation override | native: Tauri topmost/nonfocusable/skip-taskbar window plus checked tool-window style policy | native: Tauri/GTK borderless nonfocusable window; X11 stacking/sticky/taskbar hints; Wayland compositor controls stacking/workspaces |
| `CardWindow::set_visible` | Show or hide the card without requesting focus. | native: panel show/hide; refreshes inactive WebKit tracking on show; missing panel does nothing | native: shared default calls Tauri show/hide; missing window does nothing; errors ignored | native: shared default calls Tauri show/hide; missing window does nothing; errors ignored |
| `CardWindow::close` | Destroy the card and webview. | native: shared default calls Tauri destroy; missing window returns `Ok(())`; destroy errors propagate | native: shared default calls Tauri destroy; missing window returns `Ok(())`; destroy errors propagate | native: shared default calls Tauri destroy; missing window returns `Ok(())`; destroy errors propagate |
| `CardWindow::app_active` | Read activation diagnostics. | native: reads NSApplication active state and regular activation policy | fallback: returns empty string | fallback: returns empty string |
| `desktop_geometry::global_coordinates` | Declare whether global geometry is usable. | fallback: default returns `true` | fallback: default returns `true` | native: checks actual GTK display backend once; X11 returns true, Wayland false |
| `desktop_geometry::unit` | Return desktop units per logical pixel. | fallback: returns `1.0` (AppKit points) | native: Tauri scale factor, or `1.0` on error | native: Tauri scale factor, or `1.0` on error |
| `desktop_geometry::pointer` | Read the pointer in desktop coordinates. | native: Tauri cursor position divided by primary-monitor scale; failed query returns `None` | native: shared default reads Tauri physical cursor position; error returns `None` | native: X11 reads Tauri physical cursor position, error returns `None`; fallback: Wayland returns `None` without querying |
| `desktop_geometry::frame` | Read the card's outer desktop rectangle. | native: Tauri position/size divided by window scale; failed query returns `None` | native: shared default reads Tauri physical position/size; error returns `None` | native: X11 reads Tauri physical position/size, error returns `None`; fallback: Wayland returns `None` without querying |
| `desktop_geometry::place` | Move the card without changing focus. | native: Tauri logical position in AppKit points; error ignored | native: shared default rounds coordinates and sets Tauri physical position; error ignored | native: X11 sets rounded physical position, error ignored; fallback: Wayland does nothing |
| `desktop_geometry::display` | Select a monitor and convert its work area. | native: Tauri monitors converted to AppKit points; saved key → pointer → room → primary; exhausted queries return `None` | native: shared Tauri monitor selection in physical pixels; saved key → pointer → room → primary; exhausted queries return `None` | native: X11 uses the same physical-pixel selection; fallback: Wayland returns `None` without querying |
| `WindowVisibility::observe` | Read room shown/minimized, workspace and coverage observations. | native: Tauri shown/minimized, AppKit occlusion, CGWindowList uncovered fraction; unavailable fields are `None`, unknown on-screen defaults false | native: Tauri shown/minimized and DWM cloaking for on-screen; fallback: `uncovered = None`, unknown on-screen defaults true (no coverage measurement) | native: Tauri shown/minimized; fallback: `on_screen = None`, `uncovered = None`; X11 assumes on-screen, Wayland uses focused state (error → false) |
| `PointerEvents::watch` | Subscribe to pointer-inside changes and outside clicks. | native: 80 ms geometry polling plus NSEvent global mouse monitor (other apps only); returns subscription even if monitor unavailable | native: 80 ms geometry polling (only while the card is shown) plus three-button `GetAsyncKeyState` edges; short clicks can be missed; returns subscription | native: X11 80 ms geometry polling; fallback: no outside-click events, page leave timeout instead; Wayland returns `None`, page pointer events/timeout instead |
| Cursor maintenance inside `watch`: `set_cursor_in_background`, `keep_hand`, `arrow_after_leaving` | Maintain the clickable hand and restore the arrow after leaving. | native: runtime CGS background-cursor property and NSCursor refresh/reset; hand sometimes absent depending on what is underneath; missing CGS symbols leave arrow-only background behavior | fallback: ignores `state.hand`; page CSS owns cursor, no native refresh/reset | fallback: X11 ignores `state.hand`; Wayland installs no watcher; page CSS owns cursor, no native refresh/reset |

`observe` returns shown/minimized/on-screen false and `uncovered: None` when the room is missing. Otherwise failed
shown/minimized queries produce `None` (consumed as false). Unknown coverage is never converted to 0 or 1: the
portable visibility decision accepts it, provided the shown/minimized/on-screen checks pass. macOS measures coverage
only while those checks permit it. X11/Wayland selection follows GTK's actual backend, so XWayland uses the X11 path.

## Gaps worth considering

- `watch` on macOS: a local mouse monitor alongside the global monitor could cover clicks in Sidevoice's own room.
- `watch` on X11: sampling the root pointer's button mask alongside coordinates could add outside-button edges without a new global hook.
- `app_active` on Windows/Linux: a Tauri room-focus diagnostic could replace the empty string, clearly labelled as focus rather than app activation.
- Cursor maintenance on macOS: try the public Tauri cursor setter during polling as a fallback when the runtime CGS property is unavailable; background effectiveness needs checking.

## Card OS code still outside `src-tauri/src/platform`

References below are revision-qualified because each extraction shortens `src-tauri/src/call_controls.rs`; they
identify existing code, not proposed fixes. The final-stack shortcut-label branch remains outside platform.

- Main (`bf68c04`), `src-tauri/src/call_controls.rs:50`, `:147`, `:179`, `:202`, `:691`, `:716`, `:748`: NSPanel declaration, environment-based Wayland detection, construction/show policy and AppKit diagnostics; moved by #207/#208.
- Main (`bf68c04`), `src-tauri/src/call_controls.rs:315`, `:328`, `:347`, `:355`, `:429`, `:771`, `:780`, `:787`, `:798`, `:806`: native visibility/coverage and macOS geometry conversions; moved by #208/#209.
- Main (`bf68c04`), `src-tauri/src/call_controls.rs:1280`, `:1300`, `:1322`, `:1377`, `:1414`, `:1432`, `:1451`, `:1459`: native click monitors, activation/hover overrides, cursor and button polling; moved by #207/#210.
- #207 (`14a6248`), `src-tauri/src/call_controls.rs:132`, `:242`, `:255`, `:274`, `:282`, `:356`, `:621`, `:630`, `:637`, `:648`, `:656`: Wayland detection, visibility and geometry still inline until #208/#209; `:1136`, `:1156`, `:1193`, `:1211`, `:1230`, `:1238`: pointer/cursor code until #210.
- #208 (`335c4b3`), `src-tauri/src/call_controls.rs:235`, `:248`, `:267`, `:275`, `:349`: visibility still inline until #209; `:1063`, `:1083`, `:1120`, `:1138`, `:1157`, `:1165`: pointer/cursor code until #210.
- #209 (`27686a0`), `src-tauri/src/call_controls.rs:914`, `:934`, `:971`, `:989`, `:1008`, `:1016`: pointer/cursor code still inline until #210.
- #210 (`065369f`), `src-tauri/src/call_controls.rs:484`: `cfg!(target_os = "macos")` selects the mute-shortcut label. The same choice is at #207 `:598`, #208 `:591`, #209 `:442`, and main `:671`; it is still outside platform after all four extractions.

## Not tested yet on real machines

Windows, Linux X11 and Linux Wayland implementations/fallbacks have not yet been verified on real machines for
this stack. The operator uses macOS daily; the known hand-cursor limitation is recorded on the cursor operation.

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
