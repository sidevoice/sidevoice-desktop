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
is this user (`getpeereid` / `SO_PEERCRED`, `peer.uid-mismatch`). It reads `D/install.json` only if `D` and the file
are this user's and nobody else can write them (`install.unsafe`).

The device token travels only over the socket. The page never holds it: it gets the proxy's secret.

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
| a core answers; paired, token accepted | `running` |
| a core answers; not paired yet | `starting` |
| no core: the service's own state | `absent`, `not-installed`, `stopped-by-person`, `starting`, `backoff`, `failed`, `service-failed` as reported; `running` and `stopped` → `starting` |
| no core, no connector, no `install.json` | `absent` |

`state()` → `{state, failure?, core?, service?, calls?}`: `failure` as the service reports it (`{key, step, detail,
attempts, at, log_tail, …}`) or the app's own `{key, message}`; `core` `{pid, version, api, launch_id}`.

## Actions

Through `install.json` `command` only — an array of absolute paths, then the subcommand; never a shell, never a PATH
lookup, always a deadline (`Cli::installed` is the one place R4 extends with the bundled executable):

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

Every tunnel closes when the app quits, when the core refuses the token, and when a new pairing replaces it. Without a
pairing the proxy answers 503; without a core, 502.

## Tests

- `cargo test -p sidevoice-local-host`: the checks, strict HTTP, the identity proof (also a high-S signature), the
  store, the CLI runner, the state table, every row of the ingress table; end to end over a real socket against a
  stand-in core (`src/fake_core.rs`: HTTP, a WebSocket echo, the core's socket rules) — pairing, forwarding with the
  token swapped in, a socket spliced, the refusals, every startup case, revocation, the fallback and the actions.
- `tests/real_core.rs`: the same against sidevoice-core itself when `SIDEVOICE_CORE_PYTHON` names a Python that
  imports it (skipped otherwise).
- CI, Linux (`test` job): `examples/local-host-ci.rs serve` as the runner and `attack` as a second user — it cannot
  open `local.sock` (so neither pair nor link as a connector), read the app's pairing, or use the proxy without the
  secret; the same attempts as the first user, as a control, do get through.
- CI, macOS (probe build): the app with a temporary `HOME` against `local-host-ci core`; the vendored room's own page
  (`test/fixtures/local-host-flow.js`) sees `running` and the pairing, fetches through the proxy (WebKit's preflight,
  then the request with the token swapped in), is refused without the secret and on native routes, and opens a
  WebSocket through it; `curl` sends what a page cannot (hostile, missing, empty, `null` Origin; a wrong or rebound
  Host; no or a guessed secret).
