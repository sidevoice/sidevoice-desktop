# Reset helper handoff

`platform::reset_handoff::ResetHandoff` owns admission, launch, startup exclusion and cancellation.
The same executable runs `--sidevoice-reset-helper` before Tauri initializes. Only macOS currently offers
reset. Windows/Linux implement and compile the handoff without exposing reset or doing cleanup.
Webview lifecycle/removal and the reset transaction remain separate work.

The existing macOS transaction, models deletion, report/journal and relaunch remain in `data.rs`.
This change does not alter uninstall policy or elevate any process.

## Protocol and lifetime

1. Before transaction side effects, the app acquires `admission.lock` and `reset.lock` in checked private
   control storage. Startup must acquire both, and keeps them until setup and its journal acknowledgement
   finish. Failed setup releases them. A second reset returns busy.
2. The parent listens through `sidevoice-platform` v0.11.1, starts its own executable with the shared
   detached-child API, and sends a random 32-byte capability through the inherited stdin pipe. No secret
   goes in argv, the environment, logs or a file. No raw lock descriptor is inherited.
3. The helper connects through the shared channel (both peers must be this OS user) and presents the
   capability. The parent sends a length-prefixed JSON plan, limited to 16 KiB. The plan has no deletion
   paths. Its PID is only for existing browser-process observation, never authentication or proof of exit.
4. The parent retains `admission.lock` while the helper independently acquires `reset.lock`. The helper
   acknowledges readiness only after the caller has captured browser observations. The parent explicitly
   commits. There is no unlocked handoff interval.
5. The parent retains its guard and channel until process exit. The helper requires commit, channel EOF
   and acquisition of the parent's admission lock before returning permission to clean up. Startup attempts
   release admission if the helper's reset lock is busy, so they cannot deadlock the transfer.
6. The helper retains both locks through cleanup, releasing them before relaunch. Locks are persistent files,
   never deleted/recreated as a way of unlocking. Crashes release kernel locks; the existing transaction
   journal describes incomplete work on next startup.

Dropping the pending parent guard cancels over the channel and terminates/reaps the helper with the shared
bounded child API, while admission remains held. EOF before commit, a cancellation byte, a wrong capability,
a failed spawn or malformed/oversized plan never authorizes cleanup. A helper cannot be authorized just by
running the flag with a PID or file-descriptor number.

Handshake waits are bounded to 5 seconds; helper exit wait to 60 seconds; startup waits to 5 minutes and
accepts a cancellation flag. Helper bootstrap stdin is read on a bounded-wait thread: a manually launched
helper exits even if stdin stays open. These deadlines cover handoff, not the existing cleanup transaction.
Tauri `App::run` exits the process directly; the pending guard is retained in managed state until then.
Normal guard destruction means cancellation, not a successful handoff.

## Security per OS

| Host | Storage and transport | Process/lifetime |
| --- | --- | --- |
| macOS | Existing private per-user temporary reset directory; shared ancestry, owner/mode, no-follow and single-link checks. Shared Unix socket with peer uid checks on both ends. | Same executable, detached process group, non-inherited std file locks. Existing WebKit observation/cleanup unchanged. |
| Linux | Private sibling of the standard Sidevoice data directory, independent of `SIDEVOICE_DATA_DIR`; shared ancestry, owner/mode, no-follow and single-link checks. Shared Unix socket with peer uid checks on both ends. | Same executable, detached process group, std file locks. No reset UI or cleanup enablement. |
| Windows | Private sibling of the standard Sidevoice data directory; protected owner-only DACLs from creation, owner checks, held ancestry and reparse/hard-link refusal. Shared local-only named pipe with peer SID checks on both ends. | Same executable, windowless detached child, std file locks; no numeric HANDLE inheritance or PID authorization. No reset UI or cleanup enablement. |

The boundary is other ordinary OS users. Same-user malicious code, administrators and replacement of the
running program by that user are outside it. Shared APIs reject redirected/junction roots rather than
silently repairing them. No shell, service-manager replacement, privilege prompt or elevation is involved.

## Verification and limits

Tests are colocated with the platform interface and included in the existing `test-support/tests/portable.rs`
target, which normal macOS/Windows PR jobs already run. Native child modes use the test executable on every host:
real parent exit; startup exclusion before/during helper work; cancellation; helper and startup deadlines;
wrong capability; missing bootstrap; EOF before commit; spawn failure; oversized plan; hard-linked lock.
These exercise actual shared IPC and kernel locks, not a mocked lock implementation.

Linux protocol tests and lint, and Windows/macOS cross-target type checks are possible without a webview.
Native Windows/macOS protocol execution, another-user attack probes and installed-app reset/relaunch remain
runtime checks for those hosts. Browser process release, correct WebView2/WebKitGTK storage and portable
transaction/recovery are not established by this handoff block. A Windows helper in a kill-on-close enclosing
job does not escape that job; an early helper death prevents cleanup and leaves journal recovery to the caller.
