# Owned desktop cleanup

`platform::owned_removal::OwnedRemoval` replaces the shell's native recursive deletion with the existing
`sidevoice-platform` v0.12.2 held-root `Removal` capability. The reset transaction retains admission, writer
shutdown, connector-root validation, reporting and relaunch. The page still supplies no paths.

## Policy

- macOS keeps the exact existing Application Support, WebKit, cache, saved-state and log directories.
- Models removes only `blobs`, `models` and `partial`; the model root and unrelated siblings remain.
- Symlinks, including a selected link entry, are unlinked without visiting their targets. A link does not abort
  deletion of other owned entries. Hard links lose only the selected directory entry; other links retain the data.
  A Windows `.lnk` shortcut is an ordinary file. The shared Windows capability removes filesystem reparse entries
  without following their targets.
- Account home, model-store ancestors and the held removal parent are protected roots: links or replacements
  there cause refusal. Missing roots remain missing. Whole-list model preflight precedes all deletion.
- Cleanup never elevates, launches a shell, kills writers or schedules deletion after reboot. Windows/Linux reset
  remain unavailable: their native implementations return no production cleanup scope. Portable tests provide
  an explicit temporary scope and exercise the same default trait operations and actual native checks.

## Native guard and maintained libraries

Shared `HeldDirectory` holds and validates ancestry, and `Removal` owns the complete recursive walk, no-follow
entry handling, preflight, mount/device checks and deletion-time identity checks. No desktop recursive removal,
raw descriptor manipulation or unsafe bindings remain in this concern.

The shared removal walk anchors its mount check at the held parent. Desktop additionally rejects an account
home that is a mount root and a mount crossing between home and the parent. Its small native guard remains:
macOS compares device and mounted-at information through safe `rustix` bindings; Linux compares kernel mount
IDs, including same-device bind mounts; Windows held-directory opening rejects mounted-folder/reparse ancestry,
and the common path check rejects drive/share roots. Existing directories remain held while these checks and
removal run. A replaced ancestor is rechecked before deletion, and shared removal checks it during deletion.

As before, a malicious process with the same account or administrator privileges can race filesystem layout
changes. This is not atomic confinement against that process. Runtime errors can also leave partial cleanup;
the transaction's existing journal and retry behavior remain necessary.

## Validation and integration

The production interface is included in the existing portable fixture target. Tests use only temporary roots:
link targets outside the selected scope survive while other entries disappear; hard links preserve the external
name; model siblings remain; unsafe model children block all selected deletion; missing/foreign/redirected roots,
held-parent replacement and injected home/ancestor mount boundaries are refused. Actual privileged mount changes,
installed-browser lifetime and reset/relaunch require native integration checks.

PR #216 currently moves the reset helper and imports `data::{remove_known, remove_model_store}` after changing
only their visibility. This change removes those entrypoints: when the two PRs are integrated serially, its
macOS helper must import `platform::owned_removal::{NativeOwnedRemoval, OwnedRemoval}` and call the trait directly.
There are intentionally no old forwarding entrypoints. PR #218 owns browser storage selection and browser-writer
release, so its overlapping webview caller changes must also be reconciled by that owner. Neither pending PR's
implementation is changed here. This PR requires human integration review before merge.
