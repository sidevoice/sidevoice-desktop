# Crash reports

The apps send their own crash reports to Sentry, **on by default and never asked**. One setting, owned by the app, turns them off:
Settings → Crash reports, "Enviar automáticamente" (the default) or "No enviar" (`host.crashReports`, docs/BRIDGE.md).
Decided with the operator (sidevoice-desktop#523) and written down in the shared
[logging guideline](https://github.com/sidevoice/.github-private/blob/main/docs/logging.md), sections 9 and 10, which this follows.
Code: `shared/core/src/crash/` (no shell, tested anywhere), the command in `shared/tauri/src/crash_reports.rs`, the adapters in
`desktop/tauri/src/crash_reports.rs` and `mobile/tauri/src/crash_reports.rs`.

**Each piece sends its own, and only its own.** The app reports its own panics. It does not read, relay or acknowledge anything
of the core's or the connector's: they report themselves, under their own switch.

## The two steps

1. **At panic time** the log's panic hook (`shared/core/src/logging.rs`) tells the reporter, which writes a **record**: one small
   file in `crash-records/` of the app's data folder (`crash-<uuid>.json`). The hook sends nothing. The frames' addresses are taken
   in the hook (they are the dying thread's); a thread of its own resolves them and writes the record, and the hook waits for it
   for 3 s at most. Under "No enviar" no record is made; neither is one in a build with no DSN (nothing could send it).
2. **At the next start** a thread of its own sends each record as an event, newest first, and **deletes it once Sentry took it**
   (HTTP 2xx), so it is sent once. The first failure ends the pass (the network is likely down) and the rest wait for the next
   start. Under "No enviar" the pending records are discarded instead.

## The closed field list

`crash/report.rs` builds every event from the record's fields and nothing else; no Sentry client, scope, integration or transport
exists in the process (only the SDK's protocol types: `sentry-types`), so nothing an SDK would add (a host name, contexts,
breadcrumbs, modules, a user, an SDK name) can be in it. `server_name` is unset.

| Field | Content |
|---|---|
| `service.name` (tag, and the release's name) | `sidevoice-desktop` or `sidevoice-mobile` |
| `service.version` (the `release`, `<service.name>@<version>`) | the version that crashed (the record keeps it, so an update between the crash and the send changes nothing) |
| `os.type`, `os.name`, `host.arch` (tags) | OpenTelemetry values (`darwin`, `macOS`, `arm64`) from the log's resource |
| code location (`extra.location`) | `file:line` of the panic: a relative path (a dependency's from `registry/src/<index>/`: `tokio-1.40.0/src/x.rs`), never an absolute one (`unknown`) |
| `exception.type` | `panic` |
| the message (`exception.value`, `extra.len`) | the literal of a `&'static str` payload, else the fixed marker `<formatted panic message omitted>` with the formatted message's byte length; never its text, a prefix or a hash |
| the backtrace (`exception.stacktrace`) | each frame's raw instruction address, its function name when the binary has one (the symbol table stays: `strip` never removes it), a bare file name and a line when known |
| the loaded images (`debug_meta.images`) | only the modules that hold a frame of the stack: bare name (never a path), load address, size, debug id, code id; listed at start with `findshlibs`. What else is loaded would say which assistive technology or security software is installed, so it is never sent |
| the last 20 `WARN`/`ERROR` lines of our targets (`extra.log_tail`) | from an in-memory ring (below), times as offsets from the crash (`-12.345s`), ids re-keyed |
| the random id (`installation.id` tag) | below |
| `event_id`, `timestamp`, `level` (`fatal`), `platform` (`native`) | the record's id (a random UUID made at the crash), the crash's time, constants |

Not sent: the other resource attributes of the log's start event (accelerator, launch id), the thread name, the user, the IP,
the environment, the process's arguments, handled errors as events (the guideline sends them too, under the same switch; not
done here, see below).

### The ring of log lines

`crash/tail.rs` is a layer of the process's subscriber (`logging::init` adds it, whatever `SIDEVOICE_LOG` says). It keeps the last
20 `WARN`/`ERROR` events of our own targets (the process's list, minus `panic`, `crash_backtrace` and the page's `webview`), each
with its time, level, target, message, spans and **fields apart from the message**. Nothing is read back from the log file. It is
written with `try_lock` while the panic hook runs (a panic may come from inside a write; where it would have to wait, the entry
is dropped) and read with `try_lock` only.

At the crash the lines are rendered once: ids are replaced by tokens of that record (`d1`, `c2`, ...) by **field name**
(`device`, `session`, `turn`, ..., any `id` or `*_id`) and, whatever the name, by **shape** (a UUID, a long hex string), so
lines cannot be linked across reports; the home directory shows as `~` (the log's own writer backstop, reused). A line the
grammar of `crash/tail.rs` does not accept (a dependency's, a debug line, a dated one) is dropped when the record is read again.

### What a record file is

The file may have been changed since it was written, so reading it takes only the closed list: unknown fields refuse the record;
every text is bounded and cleaned (control and invisible format characters), the message is printable ASCII or the marker, a
file is a short relative path, a frame a symbol and a bare file name, an image a bare name and a debug id, a log line one of
ours. At most 20 records, 1 MiB together, none older than 30 days (the oldest go first); a file that is not a record is deleted,
a file that is not named like one is left alone.

## The random id

One per install of the app, **128 random bits from the OS** (never derived from hardware or an account), kept in
`crash-reports.json` of the app's data folder, **replaced after a year**, **erased when the person chooses "No enviar"** and made
anew when sending is turned back on. It is the only identifier a report carries. The page can show it
(`host.crashReports.installId()`, "ID de instalación" under "Ver qué se envía"): the id the next report carries, made on first
ask, `null` under "No enviar".

## The switch

"Enviar" is `auto` and "No enviar" `never` in the bridge. The app holds to the setting before any page has loaded; a file that
is there but cannot be read or understood is "never" for that run (the person may have chosen it), with a warning, and is left
as it is until the person chooses. Choosing "No enviar":

1. closes the reporter first (nothing more is recorded or sent, whatever happens after: the file may not be writable);
2. keeps the choice (an error means it was not kept, and the page is told);
3. deletes the pending records and the id.

### Copied to the core the app installed

The desktop app copies its choice **one way** to the core it installed on this computer, through the core's local-only endpoint
`PUT /api/local/crash-reports {"send": bool}` on the core's local socket (`LocalHost::set_crash_reports`; the core answers it
only there, never on TCP, to a paired device or to a browser; the connector follows the core's switch). When:

- **At the app's own first install** of the service: `LocalHost::on_installed` is told after a person's install whose installer
  answered `installed` (not `updated`, not `reinstalled`, never the app's own background update, never an app update that
  replaces binaries). `crash_reports::core_installed` then keeps that the app installed a core (`crash-reports.json`,
  `Reporter::local_core_installed`) and copies the switch as it is.
- **Each time the person changes the app's switch** (a change, not a repeat), once the app installed a core: `Reporter::set_mode`
  tells the copier (`Reporter::on_local_core_change`).

Never on every start, never read back: a later change made on the core (its CLI) stands until the person changes the app's
switch again, and never changes the app. A core the person installed by hand (`npx sidevoice install`) is not the app's: nothing
is copied to it. The app keeps no record of who installed a core other than this one marker, kept when the app's install
succeeds; a person's install made before this version has none, so the app does not copy to it.

The copy runs on a thread of its own, once, with the socket's timeouts: the person's click and the install's answer never wait
for a core. A core that cannot be reached is **one warning** (`desktop.crash-switch-not-copied`, "the crash report switch could
not be copied to the core on this computer; the core keeps its own") and nothing more: no retry loop; the next change of the
switch copies it again.

## Never breaks the app

Reporting is best-effort: nothing it does may stop, slow down or crash the app, and when it cannot work the app runs on with
reporting off for that run (tests: `shared/core/src/crash/tests.rs`, `shared/core/tests/crash_panic*.rs`).

- **No panic escapes.** Every entry the app calls (`Reporter::start`, `set_mode`, `installation_id`, `local_core_installed`,
  the pass, the capture) runs inside `catch_unwind`; a panic is logged with a fixed sentence and `error.type`
  `shared.crash-reporter-failed` and swallowed.
- **The panic hook cannot be broken by the reporter.** A panic inside a hook aborts the process and cannot be caught, so the hook
  takes the frames and waits (3 s at most) for a thread of its own. One capture runs at a time process-wide (a panic raised by
  the reporter, or by another thread meanwhile, is not recorded: no recursion, no deadlock), a panic of the sending thread is
  never recorded (a thread-local flag), locks are only tried, never waited for, and at most 5 records are made per run.
- **Nothing blocks the app.** `start` reads only the 4 KB setting file on the caller's thread; the pass runs on a thread of its
  own, every POST has a timeout (10 s, on a thread of its own) and the pass a budget (60 s); one pass at a time.
- **Init failures leave the app working.** A bad DSN, no data folder, an unreadable or unwritable directory, a disk error: the
  reporter is off for that run, `get`/`set` still answer the page (`set` says the choice was not kept).
- **A build with no DSN** (local, dev) records and sends nothing, whatever the setting says; the setting still works.

## Builds

- The DSN is baked in at compile time from `SIDEVOICE_SENTRY_DSN`, which the workflows set from the organisation's variable
  `vars.SENTRY_DSN` for the **release and nightly** channels (and the mobile TestFlight and signed Android builds), never for a
  `dev` build or a pull request.
- Debug files are uploaded with the organisation's secret `secrets.SENTRY_AUTH_TOKEN`, only in the step that uploads
  (`cargo xtask sentry upload`, org `twentynine-tech-sl`, project `sidevoice`, EU): `sentry-cli` is installed in a step of its own
  with `npm install --global --ignore-scripts` and no secret, and the checkouts do not persist credentials. The release
  workflow's call (`release-please.yml` → `build.yml`) uses `secrets: inherit`, so the token reaches the called workflow.
- `profile.release` keeps line tables and does not strip (`debug = "line-tables-only"`, `strip = "none"`). **The shipped binary
  keeps its function names and has no debug information.** Every Tauri build in `xtask` is `tauri build --no-bundle`, then
  `platform/debug_files.rs` splits the debug files off the binary into `target/debug-files`, then `tauri bundle`, so the
  bundler signs the stripped binary:
  - **Linux**: the binary as built is the debug file (line tables, symbol table, build id: `desktop/.cargo/config.toml` links with
    `--build-id=sha1`); the shipped one is `objcopy --strip-debug` of it. Checked: the debug file has `.debug_line` and a build id,
    the shipped binary no `.debug_line` and the same id. (The AppImage's own tooling may strip the binary again; symbolication
    does not depend on it: Sentry matches by build id.)
  - **macOS**: `dsymutil` makes the `.dSYM`, the shipped binary is `strip -S`. Checked: DWARF in the dSYM, and its UUID is the
    shipped binary's.
  - **Windows**: the linker's PDB, which the NSIS installer never includes. (An MSVC executable has no symbol table: its own
    frames have addresses only without Sentry.)
- `trim-paths` is not stable in the pinned cargo (1.98.1): the debug files carry the build's paths on Sentry only, and no path
  is in a record.

## Not done here

- **Handled errors as events.** The guideline sends `error` events of our own targets under the same switch (error type, code
  location, version, OS, id; a daily cap per `error.type`). Not built: the app reports crashes only.
- **Native crashes** (signals, aborts that leave no panic, OOM) are not captured: that needs the platform SDKs and a decision
  about memory capture.
- **The phone's re-emit** (guideline section 9, "Mobile exception"): the shared hook logs inside the hook on both apps; the phone
  does not re-emit the record at the next launch.
- **The page's preview** ("Ver qué se envía", sidevoice-web) still describes the earlier design; see the PR's follow-up list.
- The logs viewer (`LogSource`) has no source in the app yet.
