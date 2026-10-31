# Native remote pairing storage

`desktop/src/data/pairings.rs` owns the existing remote-pairing schema and maps storage errors to the existing
bridge refusal keys. The room commands and their caller checks stay in `data.rs`. The path is still Tauri's app-data
`pairings.json`; local-host's `local-host.json` is a separate store.

Filesystem operations use `sidevoice-platform` from core v0.12.2. `HeldDirectory` holds and checks the entire
ancestry; reads are bounded at 64 KiB and writes use the shared private staging, sync and atomic publication.
On Unix the shared implementation uses directory-relative operations and checks held identities again after
staging and sync, immediately before publication. A replaced ancestor causes refusal and staging-file cleanup. On Windows it
holds every ancestor without delete sharing until the operation ends. The desktop has no second atomic writer
for pairing files.

Discovery does not create directories. A write creates missing directories through `Files::ensure_private_dir`,
then opens the held directory. Existing app directories and legacy records may be readable by others but must
not be writable by them; they are checked without changing the directory's permissions. Every newly published
record is private from creation. Links/reparse points, unsafe ownership or permissions, and hard-linked records
are refused by the shared API. The shared Unix policy permits verified system-owned ancestor links (such as
macOS `/var`). Same-user/admin interference remains outside the trust boundary.

The desktop runs the same schema, permission, symlink and ancestor-replacement assertions on Linux, macOS and
Windows. The tests use the pinned `TestFiles` interface for links and access policy. The app's
`platform/pairing_storage` test concern attempts ancestor replacement: Unix renames and places a decoy, while
Windows proves the held handles deny the rename until the write finishes. The injection runs after the shared
directory is held but **before the shared writer stages the record**; it does not exercise the post-sync/pre-rename
boundary. That deterministic regression belongs to the shared writer and is
covered by [sidevoice-core#204](https://github.com/sidevoice/sidevoice-core/pull/204), released in v0.12.2:
`held_write_rechecks_ancestry_after_staging_before_publication` and
`held_write_refuses_post_staging_permission_changes_and_cleans_up`. Those native Unix and Windows tests verify
refusal or held-handle protection, preservation of original and decoy records, and staging-file cleanup.

Additional desktop tests cover read-only discovery, bounded and malformed reads, invalid writes preserving prior
credentials, atomic replacement, hard links, unchanged existing directory access policy, and grandparent replacement
with a real decoy directory before the shared write. The parent and grandparent tests verify Windows denies a rename
while the directory is held, then permits it once the handles close. The existing portable
test-support job includes this actual source module on all three hosts; no extra CI job is required.

## On the phone

`mobile/src/pairings.rs` keeps the same record (`{pairings, pendingRevokes}`, at most 64 KiB, the same refusals
`data.pairings-invalid`, `data.pairings-read` and `data.pairings-write`) as one file in the app's private files. It has
none of the desktop's held-directory machinery, because the sandbox is the boundary: no other app can open the app's data
directory, and a link or a hard link there is the app's own. Its `read` and `write` are the pair of the shared plugin's
`PairingStore` (`sidevoice-app-tauri`); the trait is implemented for it when the plugin joins the app (row 49), because
`sidevoice-mobile` does not depend on `app-tauri` (and through it on the engine and voice) before they build for the phone.

- **Where.** `PairingFile::of_app(app)` takes Tauri's app data directory: the app's folder in Application Support on iOS;
  on Android the app's whole data directory, so the file goes in its `files` folder (`Context.getFilesDir()`,
  `platform/app_files.rs`). Nothing is created until the first write.
- **Atomic write.** `atomic-write-file` writes the new record to a temporary file in the same directory, syncs it, renames
  it over the old one and syncs the directory (`openat`/`renameat` on the directory it opened first), so a crash or a power
  loss leaves the old record or the new one.
- **The result of a write is what a read returns.** The crate renames before it syncs the directory, so an error can come
  after the new record is in place. After any error the file is read: if it holds the bytes just written, the write is
  reported done (whether the rename survives a power loss at that very moment is not known); otherwise it failed, and the old
  record is what a read returns.
- **Shape.** The same check as the desktop's (`checked_pairings`, `desktop/src/data/pairings.rs`), copied: the desktop's file
  is also built without Tauri by `test-support` for the Windows and macOS fixture jobs, which cannot reach a shared copy in
  `app-tauri` without linking a webview, and the mobile crate cannot reach `app-tauri` yet. The two stay copies of each
  other until the shape has a home both can reach.
- **iOS data protection.** The record has the system's default class, Complete Until First User Authentication
  ([Apple Platform Security](https://support.apple.com/guide/security/secb010e978a/web)): readable from the first unlock
  after a restart, as a call under a locked screen needs. The app's entitlements are empty (`mobile/gen/apple`). Adding the
  Data Protection capability would raise the default to Complete, and the record would be unreadable while the phone is
  locked. No code sets the class.
- **Backups.** Undecided; the record is in them today. iOS: the file is in the app's container, so in a device backup (an
  encrypted one carries it to a new phone); `isExcludedFromBackup` on the folder would keep it out (the attribute is on the
  folder, because a rewrite makes a new file that has lost it). Android: the generated manifest says nothing about backup,
  so the system default applies and `files/` is part of Auto Backup, and a restore brings the pairings (their tokens) to
  another phone.
- **Tests** run on every host (a temporary directory): round trip and replacement, the shape, the bound, refusals that
  change nothing, no temporary file left behind, the outcome of a write that errs after the rename, a path that is a
  directory or has a file where its directory should be. A crash between the temporary file and the rename is
  `atomic-write-file`'s own test matter; nothing here fakes one.
- **Not run on a phone yet.** The device flow (write, kill the app, read) is part of the first device run.
