# The app's data on this computer

`platform::app_data` deletes everything the app keeps on this computer and, where the OS allows, the app itself.
It replaces the former `owned_removal` and `reset_cleanup` concerns. The reset transaction in `data.rs` keeps
admission, the pre-exit connector and keychain steps, the journal and report, relaunch and the uninstall decision;
it calls these functions in the reset helper after the app has exited. The page still supplies no paths, and the
reset plan carries none.

## Interface

- `delete_app_folders(browser, steps)`: common to every OS. Deletes, in order, `app-data`, `models`, `webview`,
  `cache` and `logs`, telling `steps` each name and result (the journal). `browser` is the `webview_data` proof that
  the browser writers exited; without it, browser storage fails with `reset.process-busy` and stays.
- `delete_data(browser, steps)`: everything. By default exactly `delete_app_folders`; macOS then removes the saved
  window state (`saved-state`) and the `defaults` domain (`preferences`, nothing to do when absent).
- `uninstallable()` / `uninstall()`: whether this copy can remove itself (macOS: a real `.app` bundle in a folder
  this user may write, not a disk image or translocated copy), and the removal (macOS: Finder's move to the Trash).
  Elsewhere `false` / `Unsupported`. `data.rs` refuses an uninstall before its confirmation when `uninstallable()`
  is false, deletes the data, and only after a clean deletion calls `uninstall()`.
- `check_tree(home, path)`: the read-only preflight of a protected root (the connector's data and releases, and
  browser storage before its removal).

What differs per OS is a private `Os` trait (`app_data/{macos,linux,windows}.rs`): the log folder rule,
`delete_data`, `UNINSTALLABLE`, the self-removal check and `uninstall`. Reset is still offered on macOS only
(`reset_availability`): the rest of the transaction is macOS-only.

## Which folders

The helper runs before Tauri and has no `AppHandle`, so the folders are resolved with `dirs` by the rule Tauri
documents for its `PathResolver`, with the bundle identifier `dev.sidevoice.desktop`:

| Step | Folder |
|---|---|
| `app-data` | `app_data_dir`: `<data>/<id>` |
| `models` | `sidevoice-platform`'s `data_dir::models_dir()`; only its `blobs`, `models` and `partial` children |
| `webview` | `webview_data`'s browser storage that is no other step's folder (macOS: `~/Library/WebKit/<id>`) |
| `cache` | `app_cache_dir`: `<cache>/<id>` |
| `logs` | `app_log_dir`: `~/Library/Logs/<id>` on macOS, `<local data>/<id>/logs` elsewhere |

A folder equal to or inside an earlier one is dropped (Linux: the logs are inside `app-data`; Windows: cache and
logs are inside the browser's local data folder). `app_local_data_dir` is covered as browser storage, and
`app_config_dir` is not written by the app. On macOS the result is exactly the previous list, in the same order,
and a test pins it: Application Support, the model children, WebKit, Caches and Logs. `saved-state` now runs after
`logs` instead of before it (both are plain owned folders, so nothing depends on that order), then `preferences`.

## Removal policy

Every removal goes through the shared held-root capability (`HeldDirectory` + `Removal`): a selected entry and its
whole tree are preflighted before anything is deleted, then checked again while deleting; links (and Windows
reparse entries) are unlinked, never followed; hard links lose only the selected name; a `.lnk` shortcut is an
ordinary file; mount and device changes below the held parent are refused. The model store's three children are
preflighted together before any is removed; the store and other siblings stay. Missing folders are completed steps.

The shared capability checks mounts only from the held parent down. Until it covers the rest, `app_data` refuses,
between the account home and that parent: a home that is not its own canonical path or is a volume's root, any
directory on the way that is not its own canonical path (any link), and any directory on another volume. Paths are
compared with `dunce` (no Windows verbatim prefixes) and volumes read with `file-id` (maintained, notify-rs). A
same-device bind mount on Linux is not detected by this interim check; the former Linux mount-ID probe moves to
the shared capability as a follow-up.

As before, a malicious process with the same account or administrator privileges can race filesystem layout
changes; this is not atomic confinement against it. Runtime errors can leave partial cleanup; the journal and retry
remain necessary. Cleanup never elevates, launches a shell, kills writers or schedules deletion after reboot.

This is deletion code: it requires human review before merge.
