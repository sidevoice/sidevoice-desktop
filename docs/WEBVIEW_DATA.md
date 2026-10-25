# Browser data during delete-all

`platform::webview_data::WebviewData` owns browser storage selection, writer release and safe removal.
The macOS reset helper uses it. Windows/Linux reset remains unavailable until the separate portable reset
transaction is assembled; this interface does not enable that UI or remove an installed app.

The concern module declares the documented trait, common types and OS selection. Its shared trait defaults
own capture, waiting and held-parent deletion; private common-type helpers own path validation and process
association. Each of `webview_data/{macos,windows,linux}.rs` implements only the native liveness and writer
selection it needs (plus macOS admission and storage roots). Callers import the trait and selected type
directly. The interface tests stay in the concern module and run on every compiled host.

## Exact storage roots

The native caller supplies Tauri's resolved `home_dir()` and `app_local_data_dir()` values. No path is accepted
from the room page. `SIDEVOICE_DATA_DIR` does not change browser storage.

| OS | Roots authorized by this interface | Evidence |
| --- | --- | --- |
| macOS | `~/Library/Application Support/dev.sidevoice.desktop`, `~/Library/WebKit/dev.sidevoice.desktop`, `~/Library/Caches/dev.sidevoice.desktop` | The existing reset allowlist in `data.rs`; Wry's default WKWebsiteDataStore remains in use. No migration or new store identifier. |
| Windows | Tauri `app_local_data_dir()`, normally `%LOCALAPPDATA%\dev.sidevoice.desktop` (the WebView2 user data folder, including its profiles/cache) | [Tauri 2.12.0 manager](https://docs.rs/crate/tauri/2.12.0/source/src/manager/webview.rs), [path resolver](https://docs.rs/crate/tauri/2.12.0/source/src/path/desktop.rs), [Wry 0.57.0 WebView2 environment](https://docs.rs/crate/wry/0.57.0/source/src/webview2/mod.rs). |
| Linux | Tauri `app_local_data_dir()`, normally `$XDG_DATA_HOME/dev.sidevoice.desktop` or `~/.local/share/dev.sidevoice.desktop`; **both data and cache are here**, including `cookies` | The same Tauri manager and resolver; [Wry 0.57.0 WebContext](https://docs.rs/crate/wry/0.57.0/source/src/webkitgtk/web_context.rs) sets both `base_data_directory` and `base_cache_directory` to this path and the cookie file to `<path>/cookies`. There is no additional default `~/.cache/webkitgtk` deletion. |

These are the pinned dependency versions, not guesses from generic browser defaults. App-directory overrides,
if configured in a future Tauri config, must be resolved by Tauri before passing the paths. The current app has
no custom data-directory override on either webview builder. A WebView2 environment/registry override pointing
an app child at a different user data folder is refused, not silently added to the deletion allowlist.
Windows capture also requires positive evidence of a runtime using the allowlisted folder; an override that
reuses another application's browser cannot make an unobserved default store look safe to remove.

## Lifetime and integration contract

1. At reset admission, call `preflight` before confirmation, journal/report mutation, connector removal or
   credential deletion. macOS verifies the app is its own responsible process. Unsupported launch mode reuses
   `reset.app-unremovable`: the existing translated message tells the user to open Sidevoice from Applications
   and retry. No new translation key. An unavailable/failed query returns `reset.helper-failed` before deletion.
   Hold reset admission and the interprocess launch barrier. Stop new webview creation before capture. The room
   builder now takes the existing admission permit; the card is created only during startup, after the launch barrier.
2. Resolve native locations and `capture` the still-live app PID before helper readiness/parent exit. Keep the
   resulting observation in the helper. It retains PID **and start time**, and includes descendants, not just
   the browser leader. Windows also finds `msedgewebview2.exe` processes whose exact `--user-data-dir` argument
   names our folder, even when they are no longer children of the app. Other user-data folders are not selected.
   macOS also queries responsibility for same-user WebKit processes: XPC Networking/WebContent services can
   have launchd as their parent. Services charged to our app are retained by PID/start time even after the app
   exits or the responsibility chain changes. A terminal-launched app that is not itself responsible is refused
   rather than associating services from other apps charged to that terminal.
3. After app exit, `wait_released` refreshes that observation until the deadline. Every tracked PID gets an
   independent native exit check: Unix `kill(pid, 0)` must return `ESRCH`; Windows opens a synchronization handle
   and requires a signaled process (or `OpenProcess` reporting the PID does not exist). Permission/query errors
   refuse cleanup. A missing sysinfo entry or changed start time never authorizes deletion. Even a reused live
   PID conservatively blocks release; PID/start pairs still select descendants without following reused parents. It never terminates another
   browser. An unavailable/failed macOS responsibility query, unreadable Windows runtime owner/command line,
   or a failed native exit inspection refuses
   cleanup; a process timeout leaves all browser roots intact and reports `reset.process-busy`.
4. Only a successful release produces the opaque `Released` value needed by `remove`. All three macOS browser
   roots share this gate, so a lingering browser cannot rewrite cookies/cache into an already deleted app-data root.
5. `remove` opens the target and its parent through core v0.12.0 `HeldDirectory`, refuses redirected roots,
   and delegates the exact child allowlist to `Removal::remove_contents`. It creates no directories, follows no
   child links and implements no recursive deletion. Missing roots are success. Unsafe ownership, mounts and
   sharing failures remain failures for the reset journal. A partial filesystem failure can be retried.
6. Retain the launch barrier until every removal/report operation ends. Only then allow the next app to start.
   A fresh empty browser profile on relaunch is expected; old browser processes must not recreate old data.

Microsoft explicitly requires the browser and its children to exit before deleting a user data folder:
[WebView2 UDF lifecycle](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/user-data-folder#delete-user-data-folders).
The interface uses the existing maintained `sysinfo` dependency for process observation and `same-file` for
Windows folder identity, plus the released shared removal primitive. macOS adds a small dynamically resolved
binding to `responsibility_get_pid_responsible_for_pid`: sysinfo has no equivalent, and no maintained Rust
wrapper was found. A missing symbol refuses cleanup. The ABI is evidenced by
[Chromium's macOS process implementation](https://chromium.googlesource.com/chromium/src/+/24aa2a50ac03698e7ec5a11251a615b965f5874d/base/process/process_info_mac.cc).
This is a macOS SPI, not a promised public API. Independent exit checks use the existing `libc` and
`windows-sys` bindings: [signal zero / ESRCH](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/kill.2.html),
[OpenProcess](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-openprocess)
and [WaitForSingleObject](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-waitforsingleobject).
The shared core process identity API, like sysinfo enumeration, is explicitly only a hint and cannot prove exit
on inspection failure. These small native checks do not signal or terminate processes. There is no recursive walker.

The security fix chooses responsible-service tracking (option b). The pinned
[Wry 0.57.0 WKWebView clear implementation](https://docs.rs/crate/wry/0.57.0/source/src/wkwebview/mod.rs)
invokes the official asynchronous removal API but supplies an empty completion callback; Tauri's
`clear_all_browsing_data` therefore cannot serve as a completed-removal barrier. Tracking the service keeps
the existing helper protocol and deletion order without introducing a separate pre-exit reset phase.

### Other process models checked

- **Windows:** Microsoft's [process model](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/process-model)
  ties renderers/helpers to a browser process and a UDF; its UDF deletion guidance explicitly says to wait for
  the browser and all children. The browser can be shared across host applications, so it is not safe to require
  it to be our app's child. The existing exact-UDF scan plus descendant tracking covers that distinction.
- **Linux:** WebKit's [GLib launcher](https://github.com/WebKit/WebKit/blob/main/Source/WebKit/UIProcess/Launcher/glib/ProcessLauncherGLib.cpp)
  chooses `executablePathOfNetworkProcess` and launches it with `GSubprocessLauncher` (spawn/fork-exec), so the
  ordinary native WebKitNetworkProcess is a child. Bubblewrap/Flatpak have separate launch paths and intermediate
  processes; the source does not prove app ancestry for all sandbox configurations. Those configurations and
  installed runtime process trees remain unverified; reset stays hidden on Linux and Windows.

## Verification and limits

Interface tests run on each compiled host: an active owner refuses release, exact allowlist removal preserves
siblings, missing roots succeed, malformed UDF arguments refuse, and a real child through the shared lifetime implementation performs its final shutdown
write before release/removal. A sibling-writer regression injects responsibility lookup (not PPID ancestry),
exits its host, confirms release is refused while the sibling remains alive, and waits for the final shutdown
write before removing storage. Lookup failure and another responsible app are also checked. An omitted-PID/inspection-error regression
refuses release, and a live PID with a mismatched start time still blocks cleanup. Windows can also
be type-checked independently of the shell's native toolkit.

The tests do not launch a real WebView2/WKWebView/WebKitGTK engine or exercise the macOS SPI. Its availability,
Finder-launched app responsibility and XPC-service lifetime must still be verified on supported macOS releases.
Installed-browser process topology,
shutdown under renderer/network-process churn, Windows file locks, and actual non-Mac reset/relaunch remain
runtime validation requirements before enabling reset there. Process snapshots are observational, not kernel
job containment: the caller's no-new-webviews and launch-barrier obligations are essential. No guarantee is
made against same-user/admin mutation, a boundary already shared by the held-root primitive.

Shared models, credentials, saved state, logs, preferences and package uninstall remain owned by the surrounding
reset transaction; this interface does not expand their deletion scope.
