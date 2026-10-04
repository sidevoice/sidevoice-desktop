# R4 Rust-native Connector pair candidate

This is a hosted macOS arm64 candidate lane for the Rust Connector + Rust Core pair. It keeps the existing
`sidevoice` resource and `Cli::bundled` verification path; the local-host install/update/rollback transaction is not
replaced. The Rust pair SEA already contains the Connector runtime and closed native Core archive, so installing the
app does not fetch Connector or Core payloads.

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
closed manifest, builds Connector's Rust runtime and SEA from the two pinned source trees, and uses that SEA while
building the production-shaped app. It then opens the packaged app, runs the bundled SEA install/status/pair-device/
same-version no-op/uninstall smoke on the fresh hosted Mac account, and uploads the tested app plus build evidence
as a seven-day Actions artifact. Pairing codes are validated in memory and never printed. The run does not publish
a prerelease or numbered release. A same-version no-op is the safe update evidence in this lane; rollback behavior
remains covered by the existing Connector transaction tests and has no previous release to exercise on a clean
account.

## Signing readiness and distribution limits

The hosted candidate is ad-hoc signed and is useful for CI review and local dogfood only. Apple Developer ID signing
and notarization are not configured: this repository's current readiness requires an Apple Developer Program
membership, a Developer ID Application certificate, the `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD` and
`APPLE_SIGNING_IDENTITY` CI secrets, and notarization credentials described in [MACOS.md](MACOS.md). Until those
are configured and a clean-Mac Gatekeeper smoke succeeds, the Actions artifact is not a double-click-ready public
installer. It is not a versioned release.
