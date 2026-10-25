# Native remote pairing storage

`src-tauri/src/data/pairings.rs` owns the existing remote-pairing schema and maps storage errors to the existing
bridge refusal keys. The room commands and their caller checks stay in `data.rs`. The path is still Tauri's app-data
`pairings.json`; local-host's `local-host.json` is a separate store.

Filesystem operations use `sidevoice-platform` from core v0.12.1. `HeldDirectory` holds and checks the entire
ancestry; reads are bounded at 64 KiB and writes use the shared private staging, sync and atomic publication.
On Unix the shared implementation uses directory-relative operations and checks held identities. On Windows it
holds every ancestor without delete sharing until the operation ends. The desktop has no second atomic writer
for pairing files.

Discovery does not create directories. A write creates missing directories through `Files::ensure_private_dir`,
then opens the held directory. Existing app directories and legacy records may be readable by others but must
not be writable by them; they are checked without changing the directory's permissions. Every newly published
record is private from creation. Links/reparse points, unsafe ownership or permissions, and hard-linked records
are refused by the shared API. The shared Unix policy permits verified system-owned ancestor links (such as
macOS `/var`). Same-user/admin interference remains outside the trust boundary.

The three original pairing tests are retained in the store module, with the Unix attack cases now running on
Linux as well as macOS. The rename injection runs after the shared directory is held and before shared publication;
the shared crate owns its internal staging and rename tests. Additional tests cover read-only discovery, bounded
and malformed reads, invalid writes preserving prior credentials, atomic replacement, hard links, unchanged
existing directory permissions, and grandparent replacement with a real decoy directory. A Windows test attempts
to rename both the parent and grandparent during publication, then verifies rename succeeds once handles close.
The existing portable test-support job includes this actual source module on all three hosts; no extra CI job is
required.
