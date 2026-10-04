# R4 Rust-native Connector pair candidate

This is a hosted macOS arm64 candidate lane for the Rust Connector + Rust Core pair. It keeps the existing
`sidevoice` resource and `Cli::bundled` verification path; the local-host install/update/rollback transaction is not
replaced. The Rust pair SEA already contains the Connector runtime and closed native Core archive, so installing the
app does not fetch Connector or Core payloads.

The bundled local install/update interface is vendored from reviewed sidevoice-web main commit
`4ae0911d4946db84f9523489966f18d1b137be51`, recorded in `ui/voice/web-source.json`. It is the same tree as reviewed
Web PR #42 head `8d56c1bb93a8c6e0b9a1f06d521d0413e721c572`; the remote-machine pairing route remains available
separately.

## Exact source inputs

[`src-tauri/r4-native-pair-source-pin.json`](../src-tauri/r4-native-pair-source-pin.json) pins the Connector
protected-main merge SHA and the Core source SHA plus Cargo lock digest. The Connector source is pinned to PR #56's
protected-main merge commit (`435fd315e657a4f1372fc524f773387797195f38`), whose tree is the reviewed `9612074` tree.
The first Desktop candidate attempt used the pre-merge review SHA and exposed a CLI contract mismatch before packaging;
the corrected candidate uses the protected-main commit. Its workflow still verifies that the Connector's own Rust
Core production pin equals the pinned Core source and lockfile. Do not build from a branch name, tag, or artifact left
by an earlier run.

The Connector producer requires all three canonical Rust Core target archives in its closed manifest. The candidate
workflow therefore builds those Core source inputs on hosted runners, but packages and tests only one Desktop app:
macOS arm64. It checks out both public repositories by exact SHA with checkout credentials disabled. There is no
cross-repository PAT, source artifact download at install time, fixture SEA, Linux/Windows Desktop build, or release
publication in this lane.

## Pair identity and install behavior

The native pair uses `connector-pin.json` schema 2, discriminated by `native_pair.runtime_kind` and
`native_pair.core_kind` set to `rust-native-v1`. Its canonical manifest bytes must match the Connector/Core
producer's URL-free closed schema, all target archive records, source SHA, Cargo lock digest and selected Mac
archive. `pair_id` binds the built Rust Connector runtime SHA-256 to that selected Core build. The existing schema 1
Python bundle pin remains a separate validation path; a Rust archive is never represented as a Python wheel or
Python bundle URL.

At runtime, the bundled SEA is checked against the exact executable size and SHA, arm64 architecture, macOS
signature, `--version --json` and `metadata --json`. The native Core fields from metadata must keep the Python
manifest SHA null and asset list empty; source, archive and pair identities are retained in the native pair pin.
Installed `R/current/release.json` metadata projects `runtime_kind` and `pair_id` into the desktop version decision.
A same-version runtime/pair change is eligible for update after the existing version, API, link and Core downgrade
checks; an installed newer Core is never replaced by this candidate. The app continues to invoke one bundled
installer transaction.

The manual `build` workflow input `r4-native-pair-candidate=true` runs the exact Core source builds, creates the
closed manifest, builds Connector's Rust runtime and SEA from the two pinned source trees, and packages the
production app. It also builds a CI-only probe variant with the same vendored web assets, pin, and SEA. On the fresh
hosted Mac account, the production app is opened first; the probe variant then loads the same packaged page and
clicks its local-install CTA. The page test verifies that the remote pairing dialog remains available, the local
install and progress complete through the Desktop bridge, the paired local device can reach Core through the page
proxy, and an explicit same-version update is a safe no-op. The smoke then verifies Connector/Core service status
and uninstalls the pair. The probe app is never uploaded; the production app, build evidence, checksums, and
secret-free smoke evidence are uploaded as a seven-day Actions artifact. No pairing code or token is logged. The
run does not publish a prerelease or numbered release. A same-version no-op is the safe update evidence in this
lane; rollback behavior remains covered by the existing Connector transaction tests and has no previous release
to exercise on a clean account.

This remains a **candidate-only** lane. The normal `build:mac` and default main Mac job still validate the checked-in
`src-tauri/connector-pin.json`, which is deliberately pending and therefore fails closed. A green candidate run
does not mean the default main package job can build this Rust-native pair. Do not describe or integrate the
candidate as a normal main package until that production boundary is deliberately updated and independently
verified.

For the current no-release dogfood, the selected delivery strategy is to keep this lane candidate-only. The existing
main Mac job is not skipped or made green with a fixture: a main push still fails closed at the pending production
pin. The signed candidate artifact from Desktop `80372872c36798792270cf7df0432e348ac1c78a` and hosted run
`37203998670` is available for its seven-day retention window. Making a one-PR merge buildable requires a separately
reviewed change to the production package path and its exact-head Mac gate.

## Update and rollback evidence boundary

The single app-owned update path reuses the Connector transaction:

- At Connector `435fd315e657a4f1372fc524f773387797195f38`, `release.decide` treats a same-version Rust pair identity
  change as an upgrade. `packages/connector/install.mjs` stages the candidate, switches the selection only after
  quiescing the old Rust owner, verifies the new service and pair identity, and flips back to the last verified
  release on verification failure. Connector `test/test_install.mjs` covers the pair decision, Rust owner quiescence,
  and selected-release recovery/rollback paths with manager stand-ins.
- Desktop `src-tauri/local-host/src/versioning.rs` tests that a changed native `pair_id` makes an update available and
  that a newer installed Core is never downgraded. `src-tauri/src/local_host.rs` routes only `Available` through
  `Cli::bundled` and `host.install_bundled(..., true)`; `NewerInstalled`, incompatible, and incomplete metadata do
  not start an update. `src-tauri/local-host/tests/local_host.rs` verifies that an `install.rollback` result remains
  an error while the previous Core is still running.
- The packaged-page smoke in the candidate run exercises the app bridge's same-version no-op and verifies pairing
  survives it. It does **not** install an older real pair, run a changed-pair update through the app page, or force an
  app-triggered failed update and verify automatic rollback on hosted macOS.

These source and component tests support the current bounded install/no-op dogfood claim; they are not end-to-end
proof of a changed-pair update or rollback in the packaged app. Do not claim those app-level behaviors as beta-verified
until one focused hosted Mac acceptance installs a prior genuine source-built pair, updates through the packaged
bridge, then causes a deterministic post-stage failure and confirms the prior pair remains reachable. The present
candidate remains usable for the bounded local-install trial without that broader claim.

## Signing readiness and distribution limits

The hosted candidate is ad-hoc signed for controlled beta dogfood. Apple Developer ID signing and notarization are
deferred distribution work; this artifact is not a public installer or a versioned release.
