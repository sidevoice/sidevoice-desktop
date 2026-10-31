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
`data.pairings-invalid`, `data.pairings-read` and `data.pairings-write`) as one file in the app's private data
directory (the path is the app's to give). It has none of the desktop's held-directory machinery, because the
sandbox is the boundary: no other app can open the app's data directory, and a link or a hard link there is the app's own.

- **Atomic write.** `atomic-write-file` writes the new record to a temporary file in the same directory, syncs it, renames
  it over the old one and syncs the directory (`openat`/`renameat` on the directory it opened first), so a crash or a power
  loss leaves the old record or the new one. A read never creates a directory; the first write does.
- **Shape.** What makes a record pairings is the room page's contract, not the file's: the app hands the store the check
  it shares with the desktop. The check lives in `desktop/src/data/pairings.rs` today (`checked_pairings`) and has to move
  next to the `PairingStore` trait in `app-tauri` before the mobile app can use it.
- **iOS data protection.** The record has the system's default class, Complete Until First User Authentication
  ([Apple Platform Security](https://support.apple.com/guide/security/secb010e978a/web)): readable from the first unlock
  after a restart, as a call under a locked screen needs. The app's entitlements are empty (`mobile/gen/apple`). Adding the
  Data Protection capability would raise the default to Complete, and the record would be unreadable while the phone is
  locked. No code sets the class.
- **Android.** Files under `files/` are private to the app's uid. The generated manifest says nothing about backup, so the
  system default applies and the file is part of Auto Backup: a restore onto another phone brings the pairings (their
  tokens) along. Whether that is wanted is undecided.
- **Tests** run on every host (a temporary directory): round trip and replacement, the bound, refusals that change
  nothing, no temporary file left behind, a path that is a directory or has a file where its directory should be. A
  crash between the temporary file and the rename is `atomic-write-file`'s own test matter; nothing here fakes one.
- **Not run on a phone yet.** The device flow (write, kill the app, read) is part of the first device run.
