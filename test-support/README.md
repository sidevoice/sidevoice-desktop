# Desktop test support

A non-published member of the app's `src-tauri/` Cargo workspace, following the connector's separate testbench shape. Production local-host has only a dev-dependency on this crate (for adversarial archive construction).

- `fake_core`: a real P-256 identity, pairing/revocation, HTTP and WebSocket echo over shared `Channels::listen`. HTTP parsing uses `httparse`, independently of the production proxy parser.
- `fake_registry`: in-memory npm documents and arbitrary archive bytes. Its transport is portable; the local-host `Fetch` adapter is Unix-only until local-host is ported.
- `fake-connector`: one native executable on every host. A JSON sidecar configures answers, refusals, delays, stderr, descendants retaining output pipes, detached services, installer records and service state. Registry fixtures package that already-built executable and its sidecar for the current host, without a shell or Perl.
- `local-host-ci core <D>`: the stand-in core for the app probe. `serve` and `attack` retain the existing Unix two-user security scenario.

From `src-tauri/`, run `cargo test -p sidevoice-local-host -p sidevoice-desktop-test-support`. Cargo builds the fixture binaries for these integration tests; no recursive Cargo invocation or separately installed tool is needed. `cargo xtask test-support` runs the portable cases on the existing Windows and macOS PR jobs; Linux runs them with the workspace suite.

This does not port production local-host: it remains `#![cfg(unix)]`. The test support library, native CLI, and portable tests compile on Windows. Neither fixture binary is bundled or enabled by a production feature.
