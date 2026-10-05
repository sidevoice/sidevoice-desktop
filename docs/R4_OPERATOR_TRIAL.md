# R4 macOS operator trial

This is a temporary Apple-Silicon self-test build, not a beta or release. It uses the existing R4 bundled page from
`sidevoice-web` commit `4ae0911d4946db84f9523489966f18d1b137be51`, the source-built Connector at
`cdcc1a613d35bf600b8f39427195cbe4700ea6ec`, and corrected Core main at
`b2ae125453baa3634b94eefcc49879588e3b6e40` (`Cargo.lock` SHA-256
`b0e068cf34f5c1549c00add4c2d94e06af3758bb1586c2e58f192220a22b53e9`). Connector Core-pin alignment is recorded in
the artifact. The app is ad-hoc signed, not notarized. The Actions artifact expires after seven days.

## Safe first start

1. Download the operator-trial artifact from its exact GitHub Actions run. Unpack the artifact, then verify the
   contained app ZIP against `SHA256SUMS` before opening it.
2. Use a fresh macOS test account so the trial cannot select or alter another account's `~/.sidevoice` install,
   pairing, or app settings.
3. Copy `Sidevoice.app` to that account's `~/Applications`, remove quarantine from that copy with
   `xattr -dr com.apple.quarantine "$HOME/Applications/Sidevoice.app"`, then open it from Finder. Allow microphone
   access if macOS asks.
4. In the packaged R4 page, install the bundled local pair and pair the machine. The pinned Connector transaction asks
   Codex to register the selected Rust runtime. A foreign or invalid existing `sidevoice` registration is left
   untouched; no credentials are copied into the MCP entry. The bridge requires exactly one connected Codex row in
   Connector's status report and fails the install result if that is absent or unconfirmed; the reachable Core remains
   installed. Hosted smoke uses Codex CLI `0.160.0` under a disposable runner `CODEX_HOME`; it checks registration but
   does not sign in or run a model. A current-pair update may reconcile registration through Connector's same-version
   transaction. The historical `24f399` app still uses `--no-agents` and does not register Codex. The trial uses the source-built
   executable already inside the app and does not download an installer at run time. Keep one-time pairing codes out of
   screenshots, logs, and reports.
5. Try a local call. This build's hosted checks cover app/page launch, install progress, local service reachability,
   pairing projection, the page proxy, and same-version update. They do **not** assert a real room join or first-call
   end-to-end success; those remain operator self-test findings.

## Known limits

- The bundled page is the existing R4 install/pair experience, not the approved W1–W6 onboarding wizard, Agents
  discovery, or unified Settings flow. Later Web production work is not vendored here.
- The corrected Core route is in the pinned source, but this artifact's automated acceptance stops before a real
  call. Do not describe it as full-beta ready.
- Changed-pair upgrade and failed-update rollback are explicitly recorded as `not-run-operator-trial`; no rollback
  proof is claimed by this artifact.
- This artifact is for a clean-account trial only. Do not use it to replace an existing installation or as a
  versioned distribution.
