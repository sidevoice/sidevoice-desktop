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

The connector step runs only for an installed connector. With no install record, `platform::connector_service` must
prove the connector's service absent, fresh from the OS (macOS: launchd and `ps`; Linux: systemd's user manager and
the process list; Windows: Task Scheduler and the process list); a missing record alone is never that proof, and any
doubt refuses with `reset.service-failed` before anything is deleted. An installed connector on a host where the app
cannot run its CLI (Windows) refuses with `reset.unsupported-connector`.

The transaction in `data.rs` is one flow, offered on macOS, Windows and Linux. Removing the app itself stays macOS-only
(`platform::uninstall::available`), a product decision. The deletion stack (this flow, the connector step with its per-OS service observation, the Windows handoff and
trusted-handle journal) and the containment-checked open beneath the held home (`sidevoice-platform`'s
`open_beneath`, desktop#267) merge together: the safety work is complete only with all of them.

## Which folders

The helper runs before Tauri and has no `AppHandle`, so the folders are resolved with `dirs` by the rule Tauri
documents for its `PathResolver`, with the bundle identifier `ai.sidevoice.desktop`:

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

## From the previous identifier

The builds before `ai.sidevoice.desktop` were `dev.sidevoice.desktop`. At every launch, first (`legacy_identifier.rs`,
under the launch lock), each folder of the list above for the old identifier that exists is carried to the same folder
of the new one, until the carry-over has finished. `moves_from(home, previous)` pairs them, resolved by the same code
as the deletion (`Places::native` takes the identifier as a parameter; on macOS the trait's `extra_folders` adds
`~/Library/Logs/<id>` and the saved window state); it names `app-data` apart from the others. The shared model store is
not among them: no identifier names it. The local host's pairing lives in `app-data` (`local-host.json`), so it goes
with it.

- The new folder does not exist: the old one is renamed to it (siblings, so a rename on one filesystem).
- It exists and the install was never set up: the new `app-data` holds none of `settings.json`, `call-controls.json` and
  `pairings.json` (the files the app writes only when the person acts; a first start writes none, only its log, and
  the local host's `local-host.json`, which the app makes by itself when a core runs here) and the new namespace holds
  no provider key. The first-start folder is renamed to `<folder>.first-start-backup` next to it, never deleted (the web
  interface keeps preferences in its storage that no file shows), and the old one renamed into place. The backup's
  `local-host.json` is copied over the old one's: the core revoked the old install's token when the new one paired.
  This is the case of the builds that ran under the new identifier before this carry-over.
- It exists and the install is set up: the new one stays, the old one is left where it is, and the log says so.
  Two installs are never merged.
- It exists and the key store could not say whether the install was set up: nothing is replaced and no key moves; the
  question is asked again at the next launch.

**The journal** (`legacy-identifier.json`) is in the reset's control directory (`<id>-reset`), which no reset deletes.
It holds whether the install was set up, decided once, before anything moves; how far each step got, written before
the step acts (set aside, moved in, done); and whether the carry-over has finished. A launch resumes from it and never
decides again, so a partial failure cannot change what the retry does, and every phase looks at what is there, so a
crash at any point resumes where it was. The app data goes last, and only when the other folders are done. Finished
means every step is done, nothing to carry included; a step that failed is retried at the next launch. A failure is
logged and never stops the launch, and the old data is never deleted before it is somewhere else.

**A reset** (the helper, before it deletes anything) marks the journal finished, started or not: what it deletes does
not come back from the old folders. **The instance lock** (`instance.lock`, same directory) is held by every running
instance for its life: a launch that cannot take it is a second instance, whose first may have its browser store open,
and carries nothing. Not carried: macOS's `defaults` domain and the reset's control directory itself. The module stays
until 1.0 (RELEASING.md, "Legacy identifier carry-over").

## Removal policy

Every folder removal goes through the shared held-root capability (`HeldDirectory` + `Removal`): a selected entry and
its whole tree are preflighted before anything is deleted, then checked again while deleting; links (and Windows
reparse entries) are unlinked, never followed; hard links lose only the selected name; a `.lnk` shortcut is an
ordinary file; mount and device changes below the held parent are refused. The model store's three children are
preflighted together before any is removed; the store and other siblings stay. Missing folders are completed steps.
macOS's `defaults` domain (`preferences`) is not a folder: it is deleted with `/usr/bin/defaults`, outside all this,
and only when the home was acquired (below); a refused home resets no preferences either.

**The home.** A deletion acquires the account home once, before its first folder step, and every step uses that held
home. It is first opened without following a link at it (`Fs::open_read`; a link there is refused), and that handle
stays open while it is checked: a directory, its own canonical path (compared with `dunce`, no Windows verbatim
prefixes), not a volume's root (its parent on its volume, read with `file-id`), and with fixed ancestry: every
directory from the filesystem root down to the home is a real directory (no link or reparse point) that this account
could not take out and replace in the directory above (`account::may_replace`). On Unix that is a directory the
account does not own and cannot write and search (`access(2)`, so groups and ACLs count), or a sticky one whose entry
the account does not own; on Windows the system's own access check, by opening with each right: no
`WRITE_DAC`/`WRITE_OWNER` on the directory, and not both `FILE_ADD_SUBDIRECTORY` there and a way to remove the entry.
The home is then held with `Directory::open_identified`, bound to the validated handle: the held directory's identity
(device and inode; Windows volume serial and file id) must equal the validated one's, compared while both are open,
and on Windows the held directory must be no reparse point. A link or junction swapped in at the home or above it
after validation, including an empty home or parent turned into a junction in place, leads elsewhere and is refused.

The two checks answer different questions, so both stay. Identity binding makes the directory held the one validated.
Fixed ancestry makes the directory validated the account's home: without it, an account that can change a directory
above its home could move another real directory to the home's path before the deletion starts, and a handle opened
there, validated and held, would be that directory, with nothing to tell it apart.

A home this refuses is an unsupported layout and fails closed with `unsafe-directory`: no folder is touched and no
preferences are reset. For example a home under a directory the account owns (`/srv/accounts/ana/profile` with
`/srv/accounts/ana` the account's), directly in a world-writable sticky directory, or anywhere an elevated or root
process runs (it may change everything). A home on another device than its parent (its own filesystem: LUKS, a
per-user Btrfs subvolume, a network share) is refused by the volume-root policy (`volume-root`). Ordinary homes pass:
`/home/<name>` (also with `/home` its own mount), `/Users/<name>`, Fedora Atomic's `/var/home/<name>` (when the
account database records it there), `C:\Users\<name>`, and a home that is its own mount on its parent's device (a
bind mount, as systemd-homed activates its directory and fscrypt homes). A home recorded through a link
(`/home/<name>` with `/home` a link) is refused by the canonical-path check, as before. The checks assume the
account's credentials are stable and unprivileged while they run (they are this process's groups and token; set-ID
launches are not supported).

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
