# Crash reports

The apps send crash reports to Sentry, **on by default and never asked**. One setting, owned by the app, turns them off:
Settings → Crash reports, "Enviar automáticamente" (the default) or "No enviar" (`host.crashReports`, docs/BRIDGE.md).
Code: `shared/core/src/crash/` (no shell, tested anywhere), the command in `shared/tauri/src/crash_reports.rs`, the
adapters in `desktop/tauri/src/crash_reports.rs` and `mobile/tauri/src/crash_reports.rs`. Decided with the operator
(sidevoice-desktop#523); the legal reading is `default-on-crash-reports-legal.md` in the briefs.

## What is sent

- **The app's own panics**, from the log's panic hook (the phone's hook tells the same observer). Message: the literal of a
  `&'static str` payload, else the fixed marker `<formatted panic message omitted>`; never a formatted message.
- **The crash records of the machines the app is connected to** (the core's `GET /api/device/crashes`), each as an event of
  its own: `event_id` is the record's id (so two apps that read the same record send one event), the release is
  `sidevoice-core@x` or `sidevoice-connector@x`, `service.name` the piece. Nothing of this app is in it: no installation
  id, no app version, no OS of this device. On the desktop the records of this computer's own core are read. The machines
  the room page reaches over the network, and the phone's, are a follow-up (they need the page's transport).
- **Acknowledgement.** A record is acknowledged (`DELETE /api/device/crashes/{id}`, which deletes it) only after Sentry
  took the event (HTTP 2xx). A failed send leaves it for the next pass (every 5 minutes); a failed acknowledgement sends it
  again under the same `event_id`.

## The closed field list

`event.rs` builds every event from these fields and `sanitize` rebuilds it from them again just before it leaves
(`before_send`), so nothing an SDK default, integration or scope adds survives: `event_id`, `timestamp`, `level`
(`fatal`), `platform`, `release`, tags (`service.name`, `os.type`, `os.name`, `host.arch`, `thread.name`,
`installation.id`), the exception (`type`, `value`, frames with `function`, a bare `filename`, `lineno`,
`instruction_addr`), `debug_meta.images` (only the modules a frame of the stack is in: bare name, arch, address, size,
debug id; for symbolication. The list of what else is loaded would say which assistive technology or security software is
installed, so it is never sent) and, for a machine record only, `extra` (`count`, `first_at`, `log_tail`: the record's own masked warnings
and errors). No user, IP, request, breadcrumb, context, server name, module list or environment. `send_default_pii` is
off, there are no default integrations and no breadcrumbs, and the SDK has no transport of its own here.

The **installation id** is 128 random bits from the OS, kept in `crash-reports.json` in the app's data folder, replaced
after a year, and forgotten when the person chooses "No enviar". It is a tag on the app's own events only. No other user
identifier exists.

The page can show the id (`host.crashReports.installId()`, "ID de instalación" under "Ver qué se envía"): the id the next
report carries, made on first ask, `null` under "No enviar".

## When a send happens, and how long it may take

- A machine's record is posted on the pass's own thread (up to 10 s) and its outcome is the event's own (by event id): an
  app panic posted at the same time cannot make a failed record look delivered, so it is not acknowledged.
- The app's own panic is posted by a thread the panic hook starts and waits for: best-effort, at most about 2.6 s (a 2 s send budget plus 0.5 s to hear its outcome), so a slow network cannot hold a dying app up.
- The setting file: no file is "auto"; a file that is there but cannot be read or understood means "never" for that run (a
  warning with `error.type` `shared.crash-settings-unreadable`), and is left as it is until the person chooses.
- A machine's record is checked again here, by shape only: the message is printable ASCII or the marker, and every tail line is
  a warning or an error of the piece's own log with its time as an offset from the crash (`-12.345s`). The app does not
  verify that the ids in a line are tokens of the record (`d1`, `c2`): the machine makes them (sidevoice-core's `crash`
  module), the app trusts that; a line of another shape and the record is skipped.

## Never breaks the app

Reporting is best-effort: nothing it does may stop, slow down or crash the app, and when it cannot work the app runs on
with reporting off for that run. What holds, and where it is tested (`shared/core/src/crash/hardening_tests.rs`,
`shared/core/tests/crash_panic_in_reporter.rs`):

- **No panic escapes.** Every entry the app calls (`Reporter::start`, `set_mode`, `installation_id`, `capture_panic`,
  `send_machine`, `machine::sync`, each pass of the poll loop) runs inside `catch_unwind`; a panic is logged with a fixed
  sentence and `error.type` `shared.crash-reporter-failed` and swallowed. A transport that panics is contained at the gate,
  so the send just failed. Slicing in the tail grammar and the id's year arithmetic cannot panic either.
- **The panic hook cannot be broken by the reporter.** A panic inside a panic hook aborts the process and cannot be caught,
  so nothing of the reporter runs in the hook: it takes the frames (the dying thread's), hands the rest to a thread of its
  own and waits at most 2.6 s. One report is handed over at a time process-wide (a panic raised by the reporter, or by
  another thread meanwhile, is not reported: no recursion, no deadlock), a thread-local guard stops a transport that
  reports again, locks are only tried never waited for, and at most 10 own panics are reported per run.
- **Nothing blocks the app.** The client is made on a thread of its own (`start_in_background`; only the 4 KB settings file
  is read on the start path) and `never` still wins over a client that finishes late. A record is sent on the poll
  thread, at most 4 POSTs are under way at once (the next is dropped, not queued, and the record stays on the machine), and
  every POST has a timeout (10 s a record, 2 s a panic) and runs on a thread of its own.
- **Init failures leave the app working.** A bad DSN, no network, a TLS error, an unreadable or unwritable settings
  directory, or no data folder at all give `Reporter::unavailable`: `never` for the run, `capture` and `send` do nothing, the
  page's `get`/`set` still answer (`set` says the choice was not kept). The command's state is always managed, so the page
  never hits a missing one.
- **The machine poll is harmless.** An unreachable core, bad JSON, a body over 1 MiB (the local socket client's limit) or a
  slow answer (10 s timeout) is an error the pass swallows; a pass looks at 50 records at most; a record that is not ours
  is skipped. The loop runs in its own thread; a pass that panics is contained and the next comes after a doubling wait
  (up to an hour).

## What is sent, and what is not

Only **crashes**: the app's own panics, and the crash records of machines the app is connected to (the core's and the
connector's own panics, which the machine already recorded). Handled errors, warnings and logs are never sent as events:
that is the decided scope. Capturing handled errors could be an opt-in later; it is not done and nothing here prepares it.

## "No enviar"

Choosing it closes the transport's gate first (nothing handed to the transport leaves), drops the client, and forgets the
installation id. There is no queue and no cache on disk to drain: the transport posts on the caller's thread. Under it the
app does not initialise anything, does not fetch the machines' records and does not acknowledge them. The choice is kept
before it is reported done; a failed write of "No enviar" still stops sending, and the page is told the choice was not
kept. A build with no DSN (local, dev) sends nothing whatever the setting says; the setting still works.

## Builds

- The DSN is baked in at compile time from `SIDEVOICE_SENTRY_DSN`, which the workflows set from `vars.SENTRY_DSN` for the
  release and nightly channels, the TestFlight and the signed Android builds, never for a dev build or a pull request.
- Release builds keep line tables (`debug = "line-tables-only"`), split the debug info off where the platform can (dSYM,
  PDB, `.dwp`) and strip only the debug sections (`strip = "debuginfo"`), so frames carry names. `cargo xtask sentry upload`
  (`secrets.SENTRY_AUTH_TOKEN`, org `twentynine-tech-sl`, project `sidevoice`, EU) creates the release
  `<service.name>@<version>`, uploads the debug files and finalises it.
- `trim-paths` is not stable in the pinned cargo (1.98.1): file names in events are cut to their last component by
  `sanitize`, and the debug files carry the build's paths on Sentry only.

## Not done here

Native crashes (signals, aborts that leave no panic, OOM) are not captured: that needs the platform SDKs and a decision
about memory capture. The app's own report carries no log tail yet. "Ver qué se envía" and the logs viewer (sidevoice-web)
have no source in the app yet.
