# Reset helper handoff

Reset (delete all data, uninstall) is a Desktop capability. `platform::reset_handoff` owns only the
handoff — admission, launch of the same executable with `--sidevoice-reset-helper` before Tauri
initializes, the authenticated plan, the wait for the app's exit, startup exclusion, deadlines and
cancellation — and its control directory, `state_dir()`. Both are identical on every host, so it is a plain
module: no trait, no per-OS files. Reset is offered on every host, and the
handoff works on all three.

The reset transaction itself (pre-exit service and keychain steps, post-exit cleanup, report/journal,
relaunch or move to the Trash) is sequenced in `data.rs`. Its native pieces sit behind platform interfaces:
`app_data` (every folder the app keeps, the native extras and removal of the app itself; docs/APP_DATA.md) and
`webview_data` (browser storage); the journal and report go through the shared private-record writer. The flow is
the same on every host; the deletion stack (this flow, the connector step with its per-OS service observation, the Windows handoff and
trusted-handle journal) and the containment-checked open beneath the held home (`sidevoice-platform`'s
`open_beneath`, desktop#267) merge together: the safety work is complete only with all of them. This change does not alter uninstall
policy or elevate any process.

## Protocol and lifetime

1. Before transaction side effects, the app acquires `admission.lock` and `reset.lock` in checked private
   control storage. Startup must acquire both, and keeps them until setup and its journal acknowledgement
   finish. Failed setup releases them. A second reset returns busy.
2. The parent listens through `sidevoice-platform` v0.12.2, starts its own executable with the shared
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
| macOS | Private sibling of the standard Sidevoice data directory, independent of `SIDEVOICE_DATA_DIR`; shared ancestry, owner/mode, no-follow and single-link checks. Shared Unix socket with peer uid checks on both ends. | Same executable, detached process group, non-inherited std file locks. Existing WebKit observation/cleanup unchanged. |
| Linux | Private sibling of the standard Sidevoice data directory, independent of `SIDEVOICE_DATA_DIR`; shared ancestry, owner/mode, no-follow and single-link checks. Shared Unix socket with peer uid checks on both ends. | Same executable, detached process group, std file locks. Reset offered; the same cleanup runs after the handoff. |
| Windows | Private sibling of the standard Sidevoice data directory; protected owner-only DACLs from creation, owner checks, held ancestry and reparse/hard-link refusal. Shared local-only named pipe with peer SID checks on both ends. | Same executable, windowless detached child, std file locks; no numeric HANDLE inheritance or PID authorization. Reset offered; the same cleanup runs after the handoff. |

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

## Platform boundary

`reset_handoff.rs` is one module for every host: the protocol functions, `Handoff`, the shared types and
`state_dir()`, a sibling of the standard data root (`…/dev.sidevoice.desktop-reset`) on every host. The
shared checked filesystem, channel and child libraries supply what differs per OS. It adds no unsafe
operation or native binding.

The reset and report commands, the native confirmation and the exclusive admission run on every host. The
helper receives the authenticated plan on every host and cleans up after the committed exit. The order is: authenticate, observe WebKit children, await committed parent exit and
admission, clean up, release admission before relaunch. The reset report lives in the handoff's private
control directory (`state_dir()`), checked with the same private-directory API as the locks.

The transaction steps (pre-confirmation checks, service before exit, provider keys, cleanup after exit, the
relaunched app's report) are one flow in `data.rs` for every host. The only step still selected by OS there is the
fresh proof that the connector's service is absent when its install record is missing (macOS: launchd and the
process list; elsewhere no proof, so the reset refuses before deleting anything); moving it behind a platform
concern with Windows and Linux proofs is plan row 43. The provider-key step is common
(`platform::credentials`, docs/CREDENTIALS.md).

The handoff protocol tests remain portable and included by the existing native fixture target; a path
contract test also checks that control state cannot be removed with the standard data root. The macOS
cleanup adapter requires the real shell and its transaction helpers, so its compilation and existing
cleanup tests belong to the normal macOS PR job. The portable test target deliberately exercises the
actual IPC/lock protocol without linking the shell or pretending to test AppKit/browser cleanup.
