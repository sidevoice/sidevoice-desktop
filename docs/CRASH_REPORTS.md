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
`instruction_addr`), `debug_meta.images` (the app's own modules: bare name, arch, address, size, debug id; for
symbolication) and, for a machine record only, `extra` (`count`, `first_at`, `log_tail`: the record's own masked warnings
and errors). No user, IP, request, breadcrumb, context, server name, module list or environment. `send_default_pii` is
off, there are no default integrations and no breadcrumbs, and the SDK has no transport of its own here.

The **installation id** is 128 random bits from the OS, kept in `crash-reports.json` in the app's data folder, replaced
after a year, and forgotten when the person chooses "No enviar". It is a tag on the app's own events only. No other user
identifier exists.

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
