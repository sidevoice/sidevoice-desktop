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

Every folder removal goes through the shared held-root capability (`HeldDirectory` + `Removal`): a selected entry and
its whole tree are preflighted before anything is deleted, then checked again while deleting; links (and Windows
reparse entries) are unlinked, never followed; hard links lose only the selected name; a `.lnk` shortcut is an
ordinary file; mount and device changes below the held parent are refused. The model store's three children are
preflighted together before any is removed; the store and other siblings stay. Missing folders are completed steps.
macOS's `defaults` domain (`preferences`) is not a folder: it is deleted with `/usr/bin/defaults`, outside all this,
and only when the home was acquired (below); a refused home resets no preferences either.

**The home.** A deletion acquires the account home once, before its first folder step, and every step uses that held
home. It must be its own canonical path (compared with `dunce`, no Windows verbatim prefixes), not a volume's root
(its parent on its volume, read with `file-id`), and its ancestry must be fixed: every directory from the filesystem
root down to the home is a real directory (no link or reparse point) that this account could not take out and
replace in the directory above (`account::may_replace`). On Unix that is a directory the account does not own and
cannot write and search (`access(2)`, so groups and ACLs count), or a sticky one whose entry the account does not
own; on Windows the system's own access check, by opening with each right: no `WRITE_DAC`/`WRITE_OWNER` on the
directory, and not both `FILE_ADD_SUBDIRECTORY` there and a way to remove the entry. The home is then opened beneath
its held parent with `Directory::open_beneath`, so no link or reparse point is followed at the home itself: a home
swapped for a link, or (Windows) an empty home turned into a junction in place, after the checks is refused. The
parent and everything above it cannot be swapped (fixed ancestry) nor turned into a link in place (each contains the
next directory down; Windows makes a junction only of an empty directory), so the directory held is the one checked.
A home this refuses is an unsupported layout and fails closed with `unsafe-directory`: no folder is touched and no
preferences are reset. For example a home under a directory the account owns (`/srv/accounts/ana/profile` with
`/srv/accounts/ana` the account's), directly in a world-writable sticky directory, on another mount or device than its
parent (a mounted home such as systemd-homed's, a per-user Btrfs subvolume), or anywhere an elevated or root process
runs (it may change everything). Ordinary homes pass: `/home/<name>` (also with `/home` its own mount),
`/Users/<name>`, Fedora Atomic's `/var/home/<name>` (when the account database records it there), `C:\Users\<name>`.
A home recorded through a link (`/home/<name>` with `/home` a link) is refused by the canonical-path check, as before.
The checks assume the account's credentials are stable and unprivileged while they run (they are this process's
groups and token; set-ID launches are not supported).

**Below the home.** Every directory a removal works in (the held parent, the model store, a browser storage root's
parent) is reached beneath the held home with the shared `Directory::open_beneath`, one component at a time relative
to the handle before it, never by path: a link, junction or other reparse point anywhere on the way (the directory
itself included), and a directory on another device, mount (a same-device bind mount included) or volume, are
refused. A browser storage root must itself be an ordinary directory, opened beneath its parent.

`check_tree` (the connector's protected roots, and the browser storage preflight) deletes nothing: it acquires its own
home (canonical, not a volume root, but without the fixed-ancestry check) and checks the tree beneath it. No removal
relies on it: the deletion that follows uses the home it acquired itself and repeats its own checks.

A process with the same account can still change what is *inside* a held directory while it is being deleted; the
removal capability rechecks identity and refuses replaced ancestors, but this is not atomic confinement against an
administrator. Runtime errors can leave partial cleanup; the journal and retry remain necessary. Cleanup never
elevates, launches a shell, kills writers or schedules deletion after reboot.

This is deletion code: it requires human review before merge.
