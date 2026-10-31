# The local host

The core on the computer where the app runs, as the app finds it, pairs with it and lets the bundled page talk to it
(sidevoice design "Onboarding, hosts and settings" rev. 3, §4.1, §4.2, §4.7; release R1). Offered on every OS
(macOS, Windows, Linux): the app starts it at launch. On Windows it needs a connector that starts the core through Task
Scheduler (connector#307): one the CLI starts on demand would die with the CLI run's Job Object (#262). The app ships no
connector: the machine is installed with `npx sidevoice@latest install`, or by the app itself from that same npm
package, without Node («Prepare this computer»; Install, below); either way the app then runs that installation's CLI.

## Pieces

| Where | What |
|---|---|
| `desktop/local-host/` | The `sidevoice-local-host` crate: everything below, blocking IO, no Tauri, so it is tested anywhere Unix. |
| `desktop/src/local_host.rs` | Starts it with the app's paths at launch, stops it at quit, answers the room page's `local_host_*` commands. |
| `bridge/desktop-bridge.js` | `window.__sidevoiceDesktop.host.localHost` (docs/BRIDGE.md). |

## Where things are

`D` is the data directory every Sidevoice program shares, by one rule owned by `sidevoice-platform`'s `data_dir` (the app
depends on that crate, at the tag of a core release): `$SIDEVOICE_DATA_DIR`, else the OS's standard place — macOS
`~/Library/Application Support/Sidevoice`, Linux `$XDG_DATA_HOME/sidevoice`, Windows `%LOCALAPPDATA%\Sidevoice\Data` (not `%LOCALAPPDATA%\Sidevoice`, the per-user install dir). `C` = `D/core`
is the core's, the connector's releases are in `D/install`; the shared model directory (`Models`, below) is in the OS standard place, so in `D` only when `D` is not overridden (macOS: its standard place). The app writes in none of them.

Where the core and the connector listen is **read, not computed**: the core writes `endpoint` in `C/core.json`, the
connector writes `{pid, endpoint}` in `D/connector.json` once it listens (it stays after a crash: a `pid` that is not
alive makes it stale, and the app takes it for no connector). `endpoint` is the record `sidevoice-platform`'s `ipc` writes
and parses (`ipc::Endpoint`); on Unix an absolute socket path in the user's runtime directory (Linux
`$XDG_RUNTIME_DIR`, macOS `$TMPDIR`) inside a private `sidevoice-<hash>` directory. Nothing in `D`: there is no local
connector, as before.

The app's own file is `local-host.json` (0600) in its config directory:
`{fp, public_key, device_id, token, host, paired_at}`, the only copy of its device token.

The app now uses Tauri's app data directory for this file, settings, call-control placement and
remote pairings; on macOS it is `~/Library/Application Support/ai.sidevoice.desktop`. The
shared downloaded-model directory is `Models` in the OS standard place (`sidevoice-platform`'s `data_dir::models_dir`, independent of `SIDEVOICE_DATA_DIR`; on macOS `~/Library/Application Support/Sidevoice/Models`). Settings ›
Advanced can run the **installed connector's top-level** `uninstall --json` before clearing the
app's own data after exit. `localHost.serviceUninstall()` below remains a service-manager action
and does not remove connector releases, conversations or agent registrations.

## Trust

The boundary is the OS user. Before any connect to the core's socket (the one `C/core.json` names) the app checks that `C` is a directory (not a
link) owned by this user with mode & 077 = 0 (`identity.unsafe-directory`), and on connecting that the process at
the other end is this user (`sidevoice_platform::ipc`'s connect: the peer's effective uid on Unix, the user of the pipe
server's process on Windows; `peer.uid-mismatch`).
Filesystem checks and record access use `sidevoice-platform` at core **v0.11.1** (`fs::Directory`,
`HeldDirectory`, `Access`, and `Fs::trusted_program`); there is no desktop copy of those primitives. Discovery opens
existing directories only. Missing directories/records remain absence; unsafe paths remain refusals. Every directory
above `D` and the app's config directory must be root's or this user's and not writable by others (sticky, as `/tmp`,
is fine). A linked data root is refused; Unix system-owned ancestor links such as macOS `/var` remain supported.
Use an ordinary directory owned by this user if a redirected root is refused. Ancestors need search access,
not directory-listing access: safe traverse-only ancestors such as `0311` and `0711` remain usable.

`install.json` and the endpoint records are read through held, checked directories, without following record links
or blocking on special files. Records must be regular files with exactly one name. Core records and the pairing
are private; connector records and app-data parents may be readable by others, but never writable by them.
`local-host.json` keeps its location and schema: existing ordinary 0600 pairings under 0755 app-data parents load
without rewriting data or permissions. Saves use the shared atomic private writer; removals use the same checked
parent. Neither operation overwrites/removes a linked or otherwise unsafe record. An ancestor replacement now causes
a refusal instead of continuing through the old directory descriptor. Oversized records are refused, never truncated
into an apparently valid record.

The connector records its native executable in `command`; action arguments are appended when the app launches it.
The recorded executable is validated by `Fs::trusted_program`, and the resolved path is the one launched
(a system program link works; retargeting the original link after validation does not change it). On Unix the shared
validator requires execute permission: a native application without it produces `install.unsafe`, with neither the
program nor the record changed. Desktop launches native Core and Connector applications; the working macOS launch
contract also applies to Windows. Program validation is not publisher authentication or descriptor-bound execution,
and cannot prevent replacement by this same user or an administrator. `SIDEVOICE_DATA_DIR` is still the checked
directory's canonical path.

The device token travels only over the socket. The page never holds it: it gets the proxy's secret.

**What the native side does not check: a person.** The room page's commands are authorised by who calls them (the
current room window, on the app's own page), not by a person's presence. A compromised room page can therefore operate
the service, call `pairingCode()` without a click and send the code anywhere (redeeming it issues a separate, durable
device token). "On the person's click" and "shown, never
sent" are the page's conventions, not security properties enforced here; the page's integrity (bundled, local, no
navigation away) is what they rest on.

## Pairing (startup cases)

On each core launch the app sees (`GET /api/local/health`, no token, no Origin, `Host: localhost`):

- no stored pairing → `POST /api/device/local/pair {name}` (the computer's name; the core revokes the previous local
  device);
- a stored fingerprint that is not the core's → pair (the core was reset);
- the same fingerprint → `GET /api/device/identity?nonce=` must be signed by the pinned key (ECDSA P-256, P1363,
  over `sidevoice-node-identity:<nonce>`), then the stored token must be accepted; refused → `refused`, and only
  `reconnect()` pairs again;
- the token cannot be written → `DELETE /api/device/local`, `failed` with `app.storage` (retried on the next core
  launch or `reconnect()`).

A new pairing is kept only if the node it names is the core that answered health (its fingerprint, which is its key's
hash) and that key signs a fresh nonce; otherwise it is revoked at once (`identity.mismatch`). While paired, every
poll asks the core whether it still accepts the token: a revocation from another device shows within 2 s.

## The poll and the state

Every 2 s while the app runs: `node.status` on the socket `D/connector.json` names (JSON lines, 1.5 s; connecting never starts
anything); when no connector answers, `service status --json` through the CLI, at most every 10 s (it starts a
process) and right after each action; the core's health; the token check. The bridge's state:

| Observed | `state` |
|---|---|
| an install the app runs is under way (`install()`, below) | `installing`, whatever else is observed (no `failure`; `reachable` as observed) |
| the core's directory or the socket's peer fails a check | `failed` (`identity.unsafe-directory`, `peer.uid-mismatch`) |
| a core with an `api` outside the app's range (1–1), or the service reports one | `incompatible` |
| a core answers; its token refused, same fingerprint | `refused` |
| a core answers; pairing or storing failed | `failed` (`app.storage`, `identity.mismatch`, …) |
| a core answers; the service reports `not-installed`, `stopped-by-person` or `service-failed` | that state (and its `failure`): the service condition wins (F6), with `reachable: true` when the pairing works |
| a core answers; paired, token accepted | `running` |
| a core answers; not paired yet | `starting` |
| no core: the service's own state | `absent`, `not-installed`, `stopped-by-person`, `starting`, `backoff`, `failed`, `service-failed` as reported; `running` → `starting` (the app cannot reach that core yet) |
| no core, no connector, no `install.json` | `absent` |

The service's state is the connector's derived status (SEAMS rev. 2 §4: the service manager's view of the core and
connector jobs, `core-failure.json` and the core's health, read fresh): `node.status` from any connector that answers,
else `service status --json`, which computes the same object.

`reachable` is whether a core answers and the app's pairing with it works; `pairing()` is non-null exactly then,
whatever `state` says — the page uses the host when `pairing()` is non-null (SEAMS §5).

`state()` → `{state, failure?, core?, product_version, service?, calls?, attempts?, limit?, installed?, reachable}`: `failure`
as the service reports it (`{key, step, message, at, log_tail, …}`) or the app's own `{key, message}`; `core` `{pid,
version, api, launch_id}` (the core's own version); `product_version` the installed Sidevoice's version — the
`sidevoice` package, which bundles the core — as the person knows it («Sidevoice 0.8.1»), `null` when unknown or
nothing is installed; `calls` absent when unknown (the service says `null` with no core answering); `attempts` the
manager's restarts so far and `limit` its limit when it has one (systemd; launchd has none) — «(2 de 5)» only with
`limit`. The service's `since`, `window_started` and `next_retry_at` are null in rev. 2 and not passed on.

`product_version` is what `install.json`'s command answers to `--version --json` (its `version`): the release the
installation runs. Neither `node.status` nor `service status --json` carries it, and it starts a process, so it is
asked once per installed program: each poll reads `install.json` and resolves its command (no process), and the
program is asked again only when that resolves to another file (an install, an update or a rollback switches
`R/current` to another release), after each action (once any pass under way has ended), and, while unknown, at most
every 10 s. No `install.json`: `null` at once. An answer that is not `X.Y.Z[-pre][+build]` (as the page accepts it),
or is longer than 64 characters, is unknown.

`installed` is whether the service starts at the next sign-in: the connector's own `start_at_login` (in `node.status`
and `service status --json`), as it says it; absent when it does not say (a connector that predates the toggle), never
inferred from `service` or from the service's own `installed`.

## Actions

Through `install.json` `command` only — an array of absolute paths, then the subcommand; never a shell, never a PATH
lookup, always a deadline (`Cli::installed` is the one place R4 extends with the bundled executable; the one program
run from elsewhere is the package's own installer, from the install's private directory, Install below). The deadline
covers the program's exit; its output streams then have 0.5 s more to close. Each run is a tree of its own
(`sidevoice_platform::process`: a process group, a Job Object on Windows): once the program has exited, whatever of its
tree still runs (a child holding the pipes) is killed at most 0.5 s later, and on a timeout the whole tree is.
What it hands to the service manager (`service start` → launchd) or starts detached in a session of its own is not in
the group and stays.

| Bridge | Runs | Deadline |
|---|---|---|
| `start()` / `stop()` / `restart()` | `service start\|stop\|restart --json` | 30 / 45 / 30 s |
| `serviceInstall()` / `serviceUninstall()` | `service install\|uninstall --json` | 45 s |
| `setStartAtLogin(enabled)` | `service login on\|off --json` | 45 s |
| `pairingCode()` | `pair-device --json` → `{code, expires_in, reach}` | 30 s |
| `reconnect()` | a new local pairing over the socket | — |
| `revealLog()` | `/usr/bin/open -R` the app log (docs/BRIDGE.md → The app log) and `D/core.log` (else `D/connector.log`), whichever exist | — |

A CLI refusal is its `{key, message}`; the app's own: `cli.unavailable`, `cli.failed`,
`cli.timeout`, `install.unsafe`, `install.unreadable`, `log.missing`, `reveal.failed`, `unsupported`,
`service.login-unsupported`.

### Start at sign-in

`setStartAtLogin(enabled)` changes whether the service starts at the next sign-in, and only that: a running service
keeps running, a stopped one stays stopped (`serviceInstall`/`serviceUninstall` also start and stop it). It answers
the state after it, as every action; anything but a boolean is refused (`bad_request`) before anything runs.

**Depends on the connector's `service login on|off --json`** (in progress in sidevoice-connector, `service/mod.rs`):
the same reply shape as the other `service … --json` verbs (`{ok: true, state, service, …}`, or `{ok: false, error:
{key, params, message}}`). The verb is `LOGIN_VERB` in `desktop/local-host/src/host.rs`, the one place it is
written. A connector that does not know it answers `--json` with `connector.usage`, and the app refuses the call with
`service.login-unsupported`, at once; an older connector that answers no JSON at all is `cli.failed`.

The state after it reads the change back: `installed` is the connector's `start_at_login` (see `state()` above), from
the fresh pass every action ends with, and `subscribe` listeners get it as any other change. A connector that does not
report `start_at_login` leaves `installed` absent, and the page falls back to what it has (`service`).

## Install

«Prepare this computer» (`install(onProgress)`, `cancel(job)`; docs/BRIDGE.md → The local host) does what
`npx sidevoice@latest install` does, without Node: it fetches the npm package itself and runs the installer inside it
(`desktop/local-host/src/{npm,tarball,install}.rs`; the steps in `host.rs`). On every OS; one install at a
time (`install.busy`), and while it runs the state is `installing`.

| Step | What | Cancellable |
|---|---|---|
| `download` | `GET https://registry.npmjs.org/sidevoice/latest` → its `version`, and in its `optionalDependencies` this computer's platform package, `@sidevoice/sidevoice-<os>-<cpu>` in npm's names (`darwin-arm64` here), which must be listed at that same version; `GET https://registry.npmjs.org/<package>/<version>` → `dist.tarball` and `dist.integrity`; the tarball into a fresh private directory, its bytes as progress | yes |
| `verification` | the tarball's SHA-512, computed as it came, against the integrity's `sha512-…` | yes |
| `staging` | the tarball unpacked (rules below), then `package/bin/sidevoice-connector install --json` run from it, with `SIDEVOICE_DATA_DIR` = `D`: it stages the release under its own root with its core, writes `install.json`, defines and starts the service jobs, verifies them within 60 s (an update that does not start is rolled back) and registers the agents | until the installer starts |
| `service-start` | from when the installer has written `install.json` (its staging done; seen as the file changing) until it answers | no |
| `pairing` | the app pairs with the new core as on any core launch (Pairing, above), and answers the state once it is `reachable`, or after 30 s with whatever it is then | no |

The private directory is removed whatever happens: on success, on a failure, on a cancel.

**Trust.** The registry is the trust, as for `npx`: every request is HTTPS (rustls with the platform's verifier, the
system's proxy, the client the engine's downloads and the updater already use) to `registry.npmjs.org`, and no redirect
is followed. The tarball's URL, read from the registry's own document, must be
`https://registry.npmjs.org/<package>/-/<file>.tgz` — that host, that package's path, a plain file name: no other
host, port, user, query, fragment, escape or dot segment (`install.authenticity`, `download`). The platform package's
name is the app's (from its OS and CPU), never the document's; versions are checked as `X.Y.Z[-pre][+build]` before
they go into a URL. The integrity must hold exactly one SHA-512 (`manifest` otherwise), and a tarball whose digest is
not it is refused before anything of it is unpacked (`install.checksum`). The documents may be 1 MiB, the tarball 512
MiB (`download-size`) and 15 min; any one read 30 s. Not checked: the registry's signatures and provenance
attestations (npm's ECDSA `dist.signatures`, sigstore) — what `npm install` checks by default is the integrity, as
here.

**Unpacking.** Into the private directory — a fresh `sidevoice-install-XXXXXX` made private (0700) in the system's temporary directory, then checked as the
app's own directories are (this user's, private, every directory above it root's or this user's and closed to others,
sticky as `/tmp` allowed; `install.unsafe`) — entry by entry, written by hand: every entry a plain file or directory
below `package/`. Refused (`install.authenticity`, with the check): a path that is absolute, has an empty, `.` or `..`
segment, a backslash or a control character, or is not below `package/` — GNU long names and pax `path` records are the
entry's name and checked the same (`archive-path`); a symbolic or hard link (`archive-link`); a device, a FIFO, a sparse
file, anything else, or an archive that does not decode (`archive-type`); more than 1 GiB or 10 000 entries
(`archive-size`). Files are created anew (`O_EXCL`, no link followed) 0600, or 0700 where the archive marks them
executable; directories 0700; nothing else of an entry (owner, times) is applied. An entry that is already there is
refused (`archive-path`). No link is ever created, so a path built from checked segments stays inside. The package must
carry `bin/sidevoice-connector` as a regular file (`install.no-bundle`), which is always written 0700, and run only
once `Fs::trusted_program` accepts it.

**The installer** runs as the CLI does (Actions, above): its absolute path, no shell, a process group of its own,
`SIDEVOICE_DATA_DIR` = `D`, the rest of the app's environment as it is (the agents it registers are found with the
app's `PATH`). It has 3 min (`cli.timeout`), which covers its own bounds: 30 s for the install lock, 30 s for the
staged binary's self-check, 60 s to verify the service and 60 s more to verify a rollback; past them it is killed with
its group (a release it was staging is cleaned by the next install, which the installer says is the recovery). Its
`{ok: false, error: {key, params, message}}` reaches the page under the key sidevoice-web knows where one means the same
(docs/BRIDGE.md lists them: `install.rolled-back` → `install.rollback`, `install.verify-failed` →
`service.start.failed`, `core.package-missing` → `install.no-bundle`, `core.package-mismatch` → `install.authenticity`
`manifest`, `service.busy` → `install.busy`, `connector.failed` → `install.failed`), any other key as it is; with its
params, its message and its last 20 lines of error output (`log_tail`; none when it was killed at its deadline, until
`sidevoice_platform::process` keeps the output of a run it kills).

**After it.** The state is read fresh (the installed version too). If the app's pairing had been refused (`refused`),
it pairs again, as `reconnect()` does: the person asked for this computer. A core the app cannot reach within 30 s
does not fail the install; the state says what is there.

## The proxy

`127.0.0.1:<ephemeral>` for the app's lifetime, with a 32-byte secret made per launch and kept in memory only. The
page's pairing for the local host is

```json
{ "fp": "…", "public_key": "…", "device_id": "…", "token": "<the proxy's secret>",
  "urls": ["http://127.0.0.1:<port>"], "host": "…", "local": true }
```

Each request is checked against design §4.1's table (`desktop/local-host/src/proxy.rs`):

| Request | Rule |
|---|---|
| any | `Host` exactly `127.0.0.1:<port>`, else 421; `Origin` present and `tauri://localhost` or `http://tauri.localhost`, else 403 (missing, empty, `null`, other) |
| `OPTIONS` + `Access-Control-Request-Method` | answered by the proxy, never forwarded, no secret: GET/POST/PUT/PATCH/DELETE, headers ⊆ `authorization, content-type, accept`; 204, `Vary: Origin`, `Max-Age: 600`, `Access-Control-Allow-Private-Network: true` when asked; anything else 403 |
| other HTTP | `Authorization: Bearer <secret>` (constant time), else 401 with the CORS headers; forwarded with the device token instead, `Origin` unchanged, the core's answer as it is |
| WebSocket | `sidevoice.token.<secret>` among the offered subprotocols, else 401; rewritten to the device token; 101 spliced both ways |
| `/api/local*`, `/api/device/local*`, `/api/connectors/link*` | 404, after percent-decoding and in any case; a path that decoding makes ambiguous (`//`, dot segments, `\`, bad escapes) is 400 |

Also refused: a security header sent twice (`Host`, `Origin`, `Authorization`, the preflight's, `Upgrade`,
`Sec-WebSocket-Protocol`, `Content-Length`), a head that does not parse strictly (bare CR/LF, folded lines, not
HTTP/1.1, not origin-form), a chunked request body (411). No redirect is followed and nothing is cached.

**One request per connection**: each head is read, checked and rewritten on its own and the connection closes after
the answer (`Connection: close` both ways). The proxy never has to find where one request ends and the next begins —
where an unchecked request could ride behind a checked one — and keeps no state between requests; the cost is a
loopback handshake per request.

A client has 5 s for its whole head (one deadline, not one per read). At most 64 connections may be still sending
their head — none of them authenticated yet. Room for a new one (beyond that, or when all 256 slots are taken) is
made only by closing one past its head deadline or silent for 1 s; a head still arriving is never cut short to admit a
newcomer, which is refused instead. Clients that never finish a head therefore hold a slot for at most 5 s, and only
while they keep sending. A refused client's leftover input
is read for at most 1 s before closing. An upgrade is framed like any request (no `Transfer-Encoding`, one
`Content-Length`, and no body: 400 / 411), and nothing the client sent behind its upgrade head goes to the core before
the core's 101 — bytes already sent with the head are refused (400).

Every tunnel closes when the app quits, when the core refuses the token, and when a new pairing replaces it. Without a
pairing the proxy answers 503; without a core, 502.

## Tests

- `cargo test -p sidevoice-local-host -p sidevoice-desktop-test-support`: the checks, strict HTTP, the identity proof (also a high-S signature), the
  store, the CLI runner, the state table, every row of the ingress table; end to end over a real socket against a
  stand-in core (`test-support/src/fake_core.rs`: HTTP, a WebSocket echo, the core's socket rules) — pairing, forwarding with the
  token swapped in, a socket spliced, the refusals, every startup case, revocation, the fallback and the actions;
  the service condition over a healthy core; 256 clients trickling partial heads while the page gets through, the
  head deadline, a valid head arriving in pieces through connection churn; data before an upgrade's 101; the CLI's
  deadline with a child holding its pipes, a timeout with a live child, a detached service kept, a command link
  retargeted after the check; a directory swapped between check and read, links, loose modes and an ancestry others
  can write.
- The install, against a stand-in registry (`test-support/src/fake_registry.rs`: the documents and a package built on the spot,
  its installer the prebuilt native `fake-connector` executable with a JSON sidecar): the real 0.8.1 documents resolved; platforms, skewed versions, documents that are not ones,
  every tarball URL off the registry; the integrity's forms; tarballs that leave the package (`..`, absolute, a GNU long
  name, a pax `path`), links, devices, an entry twice, the limits, a cancel; a release fetched, verified, unpacked and
  installed by its installer (`install --json`, `D`), a tampered tarball never unpacked, the installer's keys mapped
  with its last lines, an installer that answers nothing or hangs, a cancel mid-download, short and oversized bodies,
  the private directory gone each time; end to end with the stand-in core, `installing` while it runs and only one at a
  time, `running` and the pairing after it, a refused pairing paired again, a failed install that frees the slot. By
  hand, over the network (`--features fetch -- --ignored the_real_registry`): the real latest release resolved,
  downloaded, verified and unpacked, its installer not run.
- CI, Linux (`test` job): `test-support/src/bin/local-host-ci.rs serve` as the runner and `attack` as a second user — it cannot
  open the core's socket (so neither pair nor link as a connector), read the app's pairing, or use the proxy without the
  secret; the same attempts as the first user, as a control, do get through.
- CI, macOS (probe build): the app with a temporary `HOME` against `local-host-ci core`; the bundled room's own page
  (`test/fixtures/local-host-flow.js`) sees `running` and the pairing, fetches through the proxy (WebKit's preflight,
  then the request with the token swapped in), is refused without the secret and on native routes, and opens a
  WebSocket through it; `curl` sends what a page cannot (hostile, missing, empty, `null` Origin; a wrong or rebound
  Host; no or a guessed secret).

## Native log reveal

The existing `reveal-log` action selects the same app/core/connector log paths through
`platform::reveal::files`, backed by the already registered official `tauri-plugin-opener`.
The plugin owns Finder, Windows Explorer, and Linux file-manager integration; no shell command is built.
Every resolved path must be valid UTF-8 before any file is revealed. A non-UTF-8 target (including one reached
through a UTF-8 symlink) returns an error instead of risking selection of a different file: opener 2.7.0 uses
lossy path conversion on macOS. This restriction applies to the common interface on every host.
Missing logs retain `log.missing`; file-manager failures retain `reveal.failed` with a localized message.
`reveal-log` does not need a local host: it always selects the app log, plus the core's (else connector's) log when a
host reports one, so it runs on every OS. The local host is offered on every OS; this adds no webview opener permissions. Interactive file-manager selection still
needs runtime verification on each OS; headless tests cover error propagation without opening a window.
