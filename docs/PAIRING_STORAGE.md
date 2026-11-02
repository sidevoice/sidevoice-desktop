# Native remote pairing storage

`desktop/tauri/src/data/pairings.rs` owns the existing remote-pairing schema and maps storage errors to the existing
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

`mobile/tauri/src/pairings.rs` keeps the same record (`{pairings, pendingRevokes}`, at most 64 KiB, the same refusals
`data.pairings-invalid`, `data.pairings-read` and `data.pairings-write`) as one file, `pairings/pairings.json`, alone in a
folder of its own in the app's private files. It has
none of the desktop's held-directory machinery, because the sandbox is the boundary: no other app can open the app's data
directory, and a link or a hard link there is the app's own. Its `read` and `write` are the pair of the shared plugin's
`PairingStore` (`sidevoice-shared-tauri`); the trait is implemented for it when the plugin joins the app (row 49), because
`sidevoice-mobile` does not depend on `shared/tauri` (and through it on the engine and voice) before they build for the phone.

- **Where.** `PairingFile::of_app(app)` takes Tauri's app data directory: the app's folder in Application Support on iOS;
  on Android the app's whole data directory, so the folder goes in its `files` folder (`Context.getFilesDir()`,
  `platform/app_files.rs`). The record is `<app files>/pairings/pairings.json`. Nothing is created until the first write,
  which creates `pairings/` private to the app (`0700` on iOS and Android, `platform/private_dir.rs`) if it is missing. The
  record has a folder of its own so that the folder is what a backup rule names (see "Backups"): nothing else is kept in
  it.
- **Atomic write.** `atomic-write-file` writes the new record to a temporary file in the same directory, syncs it, renames
  it over the old one and syncs the directory (`openat`/`renameat` on the directory it opened first), so a crash or a power
  loss leaves the old record or the new one. The directory is the record's parent, `pairings/`, and the crate does not create
  it (it opens it and fails if it is missing), so the write creates it first. The temporary file lands inside it: in
  `atomic-write-file` 0.3.1, `AtomicWriteFile::open` opens `path.parent()` as a directory descriptor and
  `create_temporary_file` (`src/imp/unix/mod.rs`) creates `.<file name>.<six letters or digits>` with `openat` on that
  descriptor, so the temporary file is `pairings/.pairings.json.<random>`. (The crate's other way, an unnamed `O_TMPFILE`
  file linked into the same directory, is behind its `unnamed-tmpfile` feature, which the app does not turn on, and would
  end up in the same folder.)
- **The result of a write is what a read returns.** The crate renames before it syncs the directory, so an error can come
  after the new record is in place. After any error the file is read: if it holds the bytes just written, the write is
  reported done (whether the rename survives a power loss at that very moment is not known); otherwise it failed, and the old
  record is what a read returns.
- **Shape.** The same check as the desktop's (`checked_pairings`, `desktop/tauri/src/data/pairings.rs`), copied: the desktop's file
  is also built without Tauri by `test-support` for the Windows and macOS fixture jobs, which cannot reach a shared copy in
  `shared/tauri` without linking a webview, and the mobile crate cannot reach `shared/tauri` yet. The two stay copies of each
  other until the shape has a home both can reach.
- **iOS data protection.** The record has the system's default class, Complete Until First User Authentication
  ([Apple Platform Security](https://support.apple.com/guide/security/secb010e978a/web)): readable from the first unlock
  after a restart, as a call under a locked screen needs. The app's entitlements are empty (`mobile/tauri/gen/apple`). Adding the
  Data Protection capability would raise the default to Complete, and the record would be unreadable while the phone is
  locked. No code sets the class.
- **Backups.** The record holds the access token to the person's computer, so it stays out of every OS backup (operator
  decision, 2026-11-01); after a restore or a move to a new phone the person pairs again, as with the provider keys
  (`docs/CREDENTIALS.md`: `ThisDeviceOnly` on iOS, a Keystore key that does not migrate on Android).
  - *iOS.* The folder `pairings/` is marked `NSURLIsExcludedFromBackupKey` (`isExcludedFromBackup`;
    `platform/backup/ios.rs`, through `objc2-foundation`, which Tauri already brings into the iOS build). A mark on a
    directory covers what is in it: Apple's guidance is "to indicate the system can exclude a group of related files from
    iCloud Backup, move those files into a directory and update the directory's `isExcludedFromBackup` resource value"
    ([Optimizing your app's data for iCloud Backup](https://developer.apple.com/documentation/foundation/optimizing-your-app-s-data-for-icloud-backup)).
    It is set when the folder is created and again, idempotently, on every write, before anything is put in the folder: the
    record and the temporary file of a write are never outside the mark, and a mark that was missed (a kill between
    creating the folder and marking it) or reset is set by the next write. The rename of a new file over the record does
    not touch the folder's mark, so there is none to redo on the file. If setting it fails the write is reported failed
    (`data.pairings-write`) and has written nothing: a read still returns the old record. The mark keeps the folder out of
    iCloud and computer backups, an encrypted one included; as Apple says, it guides the system and does not guarantee the
    item never appears in a backup.
  - *Android.* `files/pairings/` is excluded by the backup rules of the Android library in `mobile/tauri/plugin`, which
    Gradle merges into the app's manifest (`gen/android` is not edited): `res/xml/data_extraction_rules.xml`
    (`android:dataExtractionRules`, Android 12 and newer) excludes it (`<exclude domain="file" path="pairings/" />`) from
    both `cloud-backup` and `device-transfer`, and `res/xml/backup_rules.xml` (`android:fullBackupContent`, Android 11 and
    older) from the backup. A rule on a directory applies to every file in it and its subdirectories
    ([Back up user data with Auto Backup](https://developer.android.com/identity/data/autobackup)). Both also exclude the
    SharedPreferences file of the provider keys, `shared_prefs/keyring-sidevoice-providers.xml`: `android-native-keyring-store`
    1.0.0 names the file `keyring-<store name>` and the app's store is `sidevoice-providers`
    (`shared/core/src/platform/credentials/android.rs`). It holds values only that phone's Keystore key can decrypt, so a
    restored copy would be dead weight. The template's manifest sets neither `allowBackup` (default true) nor any rules, so
    the merge has no conflict and needs no `tools:replace`.
  - *Nothing left outside the rule.* The atomic write names its temporary file `.pairings.json.<six letters>` and removes it
    at once, and a kill between the two leaves it there with the record's bytes. Its name is random, so no rule could name
    it, but it is in `pairings/`, which both the iOS mark and the Android rules cover. The next write does not remove it;
    it stays inside the excluded folder.
  - *No migration.* The app has never shipped with the record at another path (the mobile app has no users), so there is
    no read of an older location.
- **Tests** run on every host (a temporary directory): round trip and replacement, the shape, the bound, refusals that
  change nothing, the folder (the record is alone in it, it is private to the owner, and nothing else is put in the app's
  files), the outcome of a write that errs after the rename, a path that is a directory or has a file where a directory
  should be, and the backup exclusion: it runs on the folder before anything is put in it (the platform call is a
  parameter, as no host has a backup), never after a write that changed nothing, and a mark that fails fails the write
  before it writes, leaving the old record. A write the process never finishes (the crate's file is created and
  forgotten, as a kill leaves it) leaves its `.pairings.json.<random>` inside `pairings/` and nothing outside it. The
  plugin's tests read the Android manifest and both rule files and check that each channel excludes `pairings/` and the
  provider keys' preferences file. A crash between the temporary file and the rename is `atomic-write-file`'s own test
  matter; nothing here fakes one.
- **Not run on a phone yet.** The device flow (write, kill the app, read) is part of the first device run, with what no host can
  see: that iOS reports the folder excluded after a write, and that an Android backup and a transfer leave `pairings/` and the
  provider keys' file out.
