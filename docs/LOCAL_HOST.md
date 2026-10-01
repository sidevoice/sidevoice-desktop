# The local host

The core on the computer where the app runs, as the app finds it, pairs with it and lets the bundled page talk to it
(sidevoice design "Onboarding, hosts and settings" rev. 3, §4.1, §4.2, §4.7; release R1). Offered on macOS only
(decision O2); the code builds and is tested on Linux too. Until R4 the app ships no connector: the machine was
installed with `npx @sidevoice/uplink install`, and the app runs that installation's CLI.

## Pieces

| Where | What |
|---|---|
| `src-tauri/local-host/` | The `sidevoice-local-host` crate: everything below, blocking IO, no Tauri, so it is tested anywhere Unix. |
| `src-tauri/src/local_host.rs` | Starts it with the app's paths at launch, stops it at quit, answers the room page's `local_host_*` commands. |
| `bridge/desktop-bridge.js` | `window.__sidevoiceDesktop.host.localHost` (docs/BRIDGE.md). |

## Where things are

`D` is the connector's data directory (`$SIDEVOICE_DATA_DIR`, else `~/.sidevoice`), `C` = `D/core` the core's. The app
writes in neither. Its own file is `local-host.json` (0600) in its config directory:
`{fp, public_key, device_id, token, host, paired_at}`, the only copy of its device token.

## Trust

The boundary is the OS user. Before any connect to `C/local.sock` the app checks that `C` is a directory (not a
link) owned by this user with mode & 077 = 0 (`identity.unsafe-directory`), and after connecting that the socket's peer
is this user (`getpeereid` / `SO_PEERCRED`, `peer.uid-mismatch`). Files are checked as they are used, not by path and reopened (`src-tauri/local-host/src/trusted.rs`): every
directory above `D` and above the app's config directory must be root's or this user's and not writable by others
(sticky, as `/tmp`, is fine) — the default `~/.sidevoice` passes, a `SIDEVOICE_DATA_DIR` under a directory others can
write does not; `install.json` is read through `D`'s opened and `fstat`-checked descriptor, opened without following a
link and checked as opened (a regular file, this user's, not writable by others; `install.unsafe`); each path in its
`command` must be root's or this user's, closed to others, with a safe ancestry too — and what runs is exactly the
path that was checked, links resolved once (a system link such as `/usr/bin/node` works; a link retargeted after the
check is not followed), with `SIDEVOICE_DATA_DIR` set to `D` as checked. `local-host.json` is read and
written the same way (the file private, 0600; written beside, synced, renamed, the directory synced).

The device token travels only over the socket. The page never holds it: it gets the proxy's secret.

**What the native side does not check: a person.** The room page's commands are authorised by who calls them (the
current room window, on the app's own page), not by a person's presence. A compromised room page can therefore operate
the service, call `pairingCode()` without a click and send the code anywhere (redeeming it issues a separate, durable
device token), or call `pairRoom()` with a room and code of its choosing. "On the person's click" and "shown, never
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

Every 2 s while the app runs: `node.status` on `D/connector.sock` (JSON lines, 1.5 s; connecting never starts
anything); when no connector answers, `service status --json` through the CLI, at most every 10 s (it starts a
process) and right after each action; the core's health; the token check. The bridge's state:

| Observed | `state` |
|---|---|
| the core's directory or the socket's peer fails a check | `failed` (`identity.unsafe-directory`, `peer.uid-mismatch`) |
| a core with an `api` outside the app's range (1–1), or the service reports one | `incompatible` |
| a core answers; its token refused, same fingerprint | `refused` |
| a core answers; pairing or storing failed | `failed` (`app.storage`, `identity.mismatch`, …) |
| a core answers; the service reports `not-installed`, `stopped-by-person` or `service-failed` | that state (and its `failure`): the service condition wins (F6), with `reachable: true` when the pairing works |
| a core answers; paired, token accepted | `running` |
| a core answers; not paired yet | `starting` |
| no core: the service's own state | `absent`, `not-installed`, `stopped-by-person`, `starting`, `backoff`, `failed`, `service-failed` as reported; `running` and `stopped` → `starting` |
| no core, no connector, no `install.json` | `absent` |

The service's state is `node.status` when a supervisor answers. A plain connector (no service) answers with
`supervisor: false`, speaking only for the core it started on demand: the service's state is then
`service status --json`'s.

`reachable` is whether a core answers and the app's pairing with it works; `pairing()` is non-null exactly then,
whatever `state` says — the page uses the host when `pairing()` is non-null (SEAMS §5).

`state()` → `{state, failure?, core?, service?, calls?, reachable}`: `failure` as the service reports it (`{key, step, detail,
attempts, at, log_tail, …}`) or the app's own `{key, message}`; `core` `{pid, version, api, launch_id}`.

## Actions

Through `install.json` `command` only — an array of absolute paths, then the subcommand; never a shell, never a PATH
lookup, always a deadline (`Cli::installed` is the one place R4 extends with the bundled executable). The deadline
covers the program's exit and both its output streams. Each run is a process group of its own: once the program has
exited, whatever of its group still runs (a child holding the pipes) is killed, and on a timeout the whole group is.
What it hands to the service manager (`service start` → launchd) or starts detached in a session of its own is not in
the group and stays.

| Bridge | Runs | Deadline |
|---|---|---|
| `start()` / `stop()` / `restart()` | `service start\|stop\|restart --json` | 30 / 45 / 30 s |
| `serviceInstall()` / `serviceUninstall()` | `service install\|uninstall --json` | 45 s |
| `pairingCode()` | `pair-device --json` → `{code, expires_in, reach}` | 30 s |
| `pairRoom(url, code)` | `pair <url> <code> --json` → `{room}` (the connector restarts the core) | 60 s |
| `reconnect()` | a new local pairing over the socket | — |
| `revealLog()` | `/usr/bin/open -R D/node-service.log` (else `D/core.log`) | — |

`pairRoom` refuses (`bad_request`) a URL that is not `http(s)://…` or a code with blanks or starting with `-`, so neither
can become an option. A CLI refusal is its `{key, message}`; the app's own: `cli.unavailable`, `cli.failed`,
`cli.timeout`, `install.unsafe`, `install.unreadable`, `log.missing`, `reveal.failed`, `unsupported`.

## The proxy

`127.0.0.1:<ephemeral>` for the app's lifetime, with a 32-byte secret made per launch and kept in memory only. The
page's pairing for the local host is

```json
{ "fp": "…", "public_key": "…", "device_id": "…", "token": "<the proxy's secret>",
  "urls": ["http://127.0.0.1:<port>"], "rv": null, "host": "…", "local": true }
```

Each request is checked against design §4.1's table (`src-tauri/local-host/src/proxy.rs`):

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

- `cargo test -p sidevoice-local-host`: the checks, strict HTTP, the identity proof (also a high-S signature), the
  store, the CLI runner, the state table, every row of the ingress table; end to end over a real socket against a
  stand-in core (`src/fake_core.rs`: HTTP, a WebSocket echo, the core's socket rules) — pairing, forwarding with the
  token swapped in, a socket spliced, the refusals, every startup case, revocation, the fallback and the actions;
  the service condition over a healthy core; 256 clients trickling partial heads while the page gets through, the
  head deadline, a valid head arriving in pieces through connection churn; data before an upgrade's 101; the CLI's
  deadline with a child holding its pipes, a timeout with a live child, a detached service kept, a command link
  retargeted after the check; a directory swapped between check and read, links, loose modes and an ancestry others
  can write.
- `tests/real_core.rs`: pairing, the proxy and upgrade framing (101; chunked, a body, early data refused) against
  sidevoice-core itself when `SIDEVOICE_CORE_PYTHON` names a Python that imports it (skipped otherwise).
- CI, Linux (`test` job): `examples/local-host-ci.rs serve` as the runner and `attack` as a second user — it cannot
  open `local.sock` (so neither pair nor link as a connector), read the app's pairing, or use the proxy without the
  secret; the same attempts as the first user, as a control, do get through.
- CI, macOS (probe build): the app with a temporary `HOME` against `local-host-ci core`; the vendored room's own page
  (`test/fixtures/local-host-flow.js`) sees `running` and the pairing, fetches through the proxy (WebKit's preflight,
  then the request with the token swapped in), is refused without the secret and on native routes, and opens a
  WebSocket through it; `curl` sends what a page cannot (hostile, missing, empty, `null` Origin; a wrong or rebound
  Host; no or a guessed secret).
