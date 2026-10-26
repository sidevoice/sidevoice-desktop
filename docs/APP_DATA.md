# The app's data on this computer

Three platform concerns serve the reset transaction in `data.rs`: `platform::app_data` deletes everything the app
keeps on this computer, `platform::uninstall` removes the app itself where the OS allows, and `platform::account`
names the account home they work in. They replace the former `owned_removal` and `reset_cleanup` concerns.
`data.rs` keeps admission, the pre-exit connector and keychain steps, the journal and report, relaunch and the
uninstall decision; it calls these in the reset helper after the app has exited. The page still supplies no paths,
and the reset plan carries none.

## Interface

`app_data`:

- `delete_data(home, browser, steps)`: the only entry for data. The app folders every OS has (below), then the
  OS's own extras. Each step's name and result goes to `steps` (the journal); `false` stops. `browser` is the
  `webview_data` proof that the browser writers exited; without it, browser storage fails with
  `reset.process-busy` and stays. `home` must still be `account::home()`, checked again before anything is
  deleted: otherwise the `app-data` step fails with `unsafe-home` and nothing is deleted.
- `check_tree(home, path)`: a plain, read-only preflight of a protected root. It stays public because the
  connector preflight in `data.rs` checks the connector's data and releases with it before the installed
  uninstaller runs; the folder deletion uses it for browser storage.
- Per OS, a private trait with one method, `delete_extra_data(home, steps)`, default nothing. macOS: its logs
  (`~/Library/Logs/<id>`, `logs`), the saved window state (`saved-state`) with the same safe removal, then the
  `defaults` domain (`preferences`, nothing to do when absent). Linux and Windows: the default.

`uninstall`:

- `available()`: whether this host can remove the app (`Os::AVAILABLE`; macOS only). `data.rs` refuses an
  uninstall before its confirmation when false.
- `uninstall()`: macOS checks the running copy (a real `.app` bundle in a folder this user may write, not a disk
  image or translocated copy) and moves it to the Trash with Finder's `NSFileManager` move; elsewhere `Unsupported`.
  `data.rs` calls it only after a clean deletion. A copy that fails the bundle check is now found then, not before
  the confirmation: its data is deleted, the app stays, and the report says `reset.app-unremovable`.
- Per OS, a private trait: `const AVAILABLE` (default `false`) and `uninstall()` (default `Unsupported`).

`account`:

- `home()`: `dirs`' home (which trusts `$HOME`, as Tauri does) only when it equals the home the OS account record
  has for this user; otherwise `unsafe-home`. Not a trait: one plain function, with the record read on every OS
  (macOS and Linux: the user database entry of the effective user via `nix`'s `getpwuid_r`; Windows:
  `GetUserProfileDirectoryW` on the process token, never `USERPROFILE`). `data.rs` takes the connector preflight's
  home from it, checks it again right before the installed uninstaller runs, and the helper resolves it again for
  the browser storage and the deletion. An alternate home supplied as `HOME` (say, a restored profile inside the
  real one) is refused before anything is deleted; a test pins that.

Reset is still offered on macOS only (`platform::host::RESET_AVAILABLE`): the rest of the transaction is macOS-only.

## Which folders

The helper runs before Tauri and has no `AppHandle`, so the folders are resolved with `dirs` by the rule Tauri
documents for its `PathResolver`, with the bundle identifier `dev.sidevoice.desktop`:

| Step | Folder |
|---|---|
| `app-data` | `app_data_dir`: `<data>/<id>` |
| `models` | `sidevoice-platform`'s `data_dir::models_dir()`; only its `blobs`, `models` and `partial` children |
| `webview` | `webview_data`'s browser storage that is no other step's folder (macOS: `~/Library/WebKit/<id>`) |
| `cache` | `app_cache_dir`: `<cache>/<id>` |
| `logs` | `app_log_dir` off macOS: `<local data>/<id>/logs` |

A folder equal to or inside an earlier one is dropped (macOS and Linux: that `logs` folder is inside `app-data`;
Windows: cache and logs are inside the browser's local data folder). `app_local_data_dir` is covered as browser
storage, and `app_config_dir` is not written by the app. On macOS the common folders and then the extras are
exactly the previous list, in the same order, and tests pin it: Application Support, the model children, WebKit,
Caches, Logs, Saved Application State, then `preferences`.

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
