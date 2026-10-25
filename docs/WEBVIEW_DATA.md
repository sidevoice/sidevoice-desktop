# Browser data during delete-all

`platform::webview_data::WebviewData` owns browser storage selection, writer release and safe removal.
The macOS reset helper uses it. Windows/Linux reset remains unavailable until the separate portable reset
transaction is assembled; this interface does not enable that UI or remove an installed app.

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

1. Hold reset admission and the interprocess launch barrier. Stop new webview creation before capture. The room
   builder now takes the existing admission permit; the card is created only during startup, after the launch barrier.
2. Resolve native locations and `capture` the still-live app PID before helper readiness/parent exit. Keep the
   resulting observation in the helper. It retains PID **and start time**, and includes descendants, not just
   the browser leader. Windows also finds `msedgewebview2.exe` processes whose exact `--user-data-dir` argument
   names our folder, even when they are no longer children of the app. Other user-data folders are not selected.
3. After app exit, `wait_released` refreshes that observation until the deadline. It never terminates another
   browser. An unreadable Windows runtime owner/command line or an uninspectable tracked start time refuses
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
Windows folder identity, plus the released shared removal primitive. It adds no native unsafe code or recursive walker.

## Verification and limits

Interface tests run on each compiled host: an active owner refuses release, exact allowlist removal preserves
siblings, missing roots succeed, malformed UDF arguments refuse, and a real child through the shared lifetime implementation performs its final shutdown
write before release/removal. Windows can also be type-checked independently of the shell's native toolkit.

The tests do not launch a real WebView2/WKWebView/WebKitGTK engine. Installed-browser process topology,
shutdown under renderer/network-process churn, Windows file locks, and actual non-Mac reset/relaunch remain
runtime validation requirements before enabling reset there. Process snapshots are observational, not kernel
job containment: the caller's no-new-webviews and launch-barrier obligations are essential. No guarantee is
made against same-user/admin mutation, a boundary already shared by the held-root primitive.

Shared models, credentials, saved state, logs, preferences and package uninstall remain owned by the surrounding
reset transaction; this interface does not expand their deletion scope.
