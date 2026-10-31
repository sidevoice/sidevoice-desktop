# Reset helper handoff

Reset (delete all data, uninstall) is a Desktop capability. `platform::reset_handoff` owns only the
handoff — admission, launch of the same executable with `--sidevoice-reset-helper` before Tauri
initializes, the authenticated plan, the wait for the app's exit, startup exclusion, deadlines and
cancellation — and its control directory, `state_dir()`. The protocol and the directory are the same on every
host; one thing differs, behind a private per-OS trait: where a running process's open files cannot be removed
(Windows), the helper also waits for the parent process itself to have terminated. Reset is offered on every host,
and the
handoff works on all three.

The reset transaction itself (pre-exit service and keychain steps, post-exit cleanup, report/journal,
relaunch or move to the Trash) is sequenced in `data.rs`. Its native pieces sit behind platform interfaces:
`app_data` (every folder the app keeps, the native extras and removal of the app itself; docs/APP_DATA.md) and
`webview_data` (browser storage); the journal and report go through the shared private-record writer and are read
back through the control directory's held, checked handle. The flow is the same on every host. The deletion stack
(this flow, the connector step with its per-OS service observation, this Windows handoff and trusted-handle journal)
and the containment-checked open beneath the held home (`sidevoice-platform`'s `open_beneath`, desktop#267) merge
together: the safety work is complete only with all of them. On Windows the journal's publication is not yet durable
across a power loss (below), and a recorded connector still needs a CLI the app can run there. This change does not alter uninstall
policy or elevate any process.

## Protocol and lifetime

1. Before transaction side effects, the app acquires `admission.lock` and `reset.lock` in checked private
   control storage. Startup must acquire both, and keeps them until setup and its journal acknowledgement
   finish. Failed setup releases them. A second reset returns busy. The same directory also holds the temporary
   carry-over from the previous identifier's journal, key records and instance lock (APP_DATA.md, "From the previous
   identifier"): `begin_reset` marks that journal finished before it clears anything, so nothing the reset deletes
   comes back.
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
   and acquisition of the parent's admission lock before returning permission to clean up. Where open files block
   removal (Windows), it also requires the parent's termination: the helper opens a synchronization-only handle to
   the plan's PID right after receiving the plan, by handle rather than PID. If the parent died and its PID was reused before
   the hold, the process held is another one, but that cannot authorize cleanup: a dead parent never receives the
   helper's readiness, so it never sends the COMMIT required before the lock and the termination. The process object is signaled only once every
   handle the parent had is closed. A hold that fails refuses; the wait shares the exit deadline. Startup attempts
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
| Linux | Private sibling of the standard Sidevoice data directory, independent of `SIDEVOICE_DATA_DIR`; shared ancestry, owner/mode, no-follow and single-link checks. Shared Unix socket with peer uid checks on both ends. | Same executable (an AppImage: the AppImage file itself, not its temporary mount), detached process group, std file locks. Reset offered. |
| Windows | Private sibling of the standard Sidevoice data directory; protected owner-only DACLs from creation, owner checks, held ancestry and reparse/hard-link refusal. Shared local-only named pipe with peer SID checks on both ends. | Same executable, windowless detached child, std file locks; no numeric HANDLE inheritance or PID authorization. Cleanup also waits for the parent's termination through a handle held from the handshake. Windowless detached relaunch. Reset offered. |

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

`reset_handoff.rs` holds everything common: the protocol functions, `Handoff`, the shared types, `launch` and
`relaunch`, and `state_dir()`, a sibling of the standard data root (`…/ai.sidevoice.desktop-reset`) on every host.
The shared checked filesystem, channel and child libraries supply most of what differs per OS. The rest is a private
`trait Os` in `reset_handoff/{macos,linux,windows}.rs`: `OPEN_FILES_BLOCK_REMOVAL` (default `false`; Windows `true`)
and the hold that observes the parent's termination (`hold`, `terminated`; honest defaults: nothing can be held). The program check is not part of it:
`installed_program` in `data.rs` holds `PROGRAM_CHECKED` (`true`; macOS `false`).
The decision (wait for termination where open files block removal, refuse when the hold fails) is in the common
file. Windows' hold is the one own `unsafe`: `OpenProcess(PROCESS_SYNCHRONIZE)` and a zero-timeout
`WaitForSingleObject`, as `webview_data` already does; nothing maintained offers a bounded wait on a process
held from a given moment (`sysinfo`'s `Process::wait` is unbounded).

The reset and report commands, the native confirmation and the exclusive admission run on every host. The
helper receives the authenticated plan on every host and cleans up after the committed exit. The order is: authenticate, observe WebKit children, await committed parent exit and
admission, clean up, release admission before relaunch. The reset report lives in the handoff's private
control directory (`state_dir()`), checked with the same private-directory API as the locks.

The transaction steps (pre-confirmation checks, service before exit, provider keys, cleanup after exit, the
relaunched app's report) are one flow in `data.rs` for every host. With no connector install record, the
connector step is skipped only on a fresh proof that the connector's service is absent, which
`platform::connector_service` reads from each OS (service manager jobs and running programs; fail-closed). The provider-key step is common
(`platform::credentials`, docs/CREDENTIALS.md).

The handoff protocol tests remain portable and included by the existing native fixture target; a path
contract test also checks that control state cannot be removed with the standard data root. The macOS
cleanup adapter requires the real shell and its transaction helpers, so its compilation and existing
cleanup tests belong to the normal macOS PR job. The portable test target deliberately exercises the
actual IPC/lock protocol without linking the shell or pretending to test AppKit/browser cleanup.

## Programs, journal and report

The helper and the relaunch start one program. Where the reset is new (Linux, Windows: `PROGRAM_CHECKED`), it is the
installed app as Tauri resolves its own binary (`tauri::process::current_binary`: the AppImage file on Linux, taken from
`APPIMAGE`, since its temporary mount goes away with the app; an environment trusted as `HOME` is, not a proof of
identity), checked by `sidevoice-platform`'s `trusted_program` (this user's or the system's, nobody else may change
it or the folders above it). It is resolved before any side effect (a refusal is `reset.helper-failed` with nothing
touched) and again right before the relaunch. macOS keeps the launch it shipped with, `current_exe` unchecked:
Tauri's resolver refuses an executable path with any link in it, which used to work. Both start through the shared detached-child API: on Windows without a console
window, and with none of the helper's standard streams.

The journal is written before each step's effect is reported, and the helper writes its `cleanup` stage before it
deletes anything: if the write fails, nothing is deleted and the journal from before the exit reports an interrupted
reset. That ordering covers a crash of the helper or the app. Surviving a power loss also needs the publication to be
durable when the write returns: on Unix the shared writer syncs the staged file and then the directory after the
rename; on Windows it flushes the staged file but renames it without write-through and has no directory flush, so the
newest record may be lost on power loss. Making that rename durable (`MOVEFILE_WRITE_THROUGH` or a flush in
`sidevoice-platform`'s writer) is open work; until then a power loss mid-reset on Windows can report an older stage.

The page's report read (`data_reset_report`) and the relaunched app's finalization read, write and remove the
records through the control directory's held handle (`HeldDirectory`). A record read must be a private regular file
with one name, never a link or reparse point (else `reset.unsafe`), of at most 16 KiB (else `reset.failed` for the
report, `reset.report-failed` for the journal); a refused record stays. Once a valid report is read, the journal
beside it is obsolete and is removed unread: any regular file with one name that others cannot write, never a link or
reparse point.
