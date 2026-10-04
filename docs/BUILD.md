# Building the macOS arm64 app

The product build is `npm run build:mac`, both locally and in Actions. It requires a **ready, reviewed**
`src-tauri/connector-pin.json`; the pending pin deliberately prevents a production build. A successful fixture
package is only evidence that Tauri carries the resource. It is not an installable R4 local-host implementation.

The isolated R4 Rust-native candidate is built by the `r4-native-pair-candidate=true` manual Actions lane and
`npm run build:mac:native-pair`. That command accepts only the exact source-built Rust pair pin and SEA staged by
the workflow, temporarily supplies them through the existing app resource map, verifies the production-shaped
bundle, and restores the pending checkout pin and prior resource on exit. See [R4_NATIVE_PAIR.md](R4_NATIVE_PAIR.md)
for exact source pins, the candidate smoke and signing limits. Do not use this candidate lane for release publication.

## Inputs and commands

Use an Apple Silicon Mac with Xcode Command Line Tools and Tauri's macOS prerequisites, Node **22.23.3**
(`.node-version`), Rust **1.99.0** (`rust-toolchain.toml`), and GitHub CLI with `gh attestation verify` (including
`--signer-digest`, `--source-digest` and `--deny-self-hosted-runners`). Node/Rust versions are shared with CI. npm and Cargo
use the committed lockfiles; the build passes `--locked` to Cargo. The web interface is already vendored into
`ui/`; its exact upstream commit is `ui/voice/web-source.json`. Build it in sidevoice-web before vendoring;
the desktop build does not silently update it or fetch an upstream branch.

```sh
npm ci
npm test
# Optional network-capable negative check of the real GitHub verifier (also run in CI):
node --test test/attestation-cli.mjs
npm run build:mac
# Optional branded disk image, including the same app build and verification:
npm run package:mac
```

The app is `src-tauri/target/packages/production/Sidevoice.app`. The optional disk image and its checksum are in
`dist/`. App packaging verifies the signature, entitlements, arm64 executable, microphone declaration, minimum
macOS, icons, absence of probe code, bundled pin and the bundled connector's digest and metadata. The disk image
check verifies the mounted app and its connector after copying it out of the image.

`dist/build-evidence.json` records the desktop commit, tracked working-tree changes, web provenance, Node/Rust/GitHub CLI,
lockfile digests, app executable digest, connector digest/SHA and core manifest digest. A dirty checkout is
explicitly recorded. Keep this evidence with the app tested. It does **not** assert the clean-account smoke below.
The build shares Cargo's dependency cache, then copies each finished app into a separate production/fixture/probe
location so a later probe build cannot replace the installable app.

This is a repeatable build from pinned source/dependency inputs, not a claim of bit-identical signed bundles:
macOS SDK/runner image and packaging timestamps are not locked. The optional DMG retains the existing
`dmgbuild==1.6.7` installation; fully hashed transitive Python dependencies remain a distribution follow-up.

## Connector trust and the temporary dogfood channel

The reviewed pin is the trust root for the connector executable's exact bytes. Before running it, the downloader:

1. Validates the manifest bytes, their digest/size, core asset identities and protocol metadata in the pin.
2. Fetches the artifact and workflow-run records from GitHub's authenticated API; checks the repository IDs,
   connector SHA, main workflow/ref, build sequence, successful production run, artifact name/ID and expiry.
   The mandatory `sidevoice-provenance.zip` artifact must belong to that same run and match its pinned size/digest.
3. Downloads through bounded HTTPS requests. Authorization goes only to the initial GitHub API request,
   never to a redirect host. Tokens and signed redirect URLs are excluded from diagnostics.
4. Checks each pinned sidecar's size/digest and extracts only the single root `sidevoice` executable. The provenance
   ZIP must contain only `sidevoice.sigstore.json`, bounded to 1 MiB after extraction. Before making the SEA
   executable, `gh attestation verify` verifies its signature, transparency evidence, SLSA subject digest and
   GitHub Actions issuer against connector's `r4-sea.yml@refs/heads/main`, pinned signer/source commit, and GitHub
   hosted runners. Missing CLI, network/trust-root failure or signature/policy mismatch fails closed.
5. Checks the SEA's pinned size/digest, native architecture, macOS signature, `--version --json` and `metadata --json`, then atomically
   replaces the resource. A failure leaves the existing resource untouched and fails the build.

The canonical download endpoint is `/repos/sidevoice/sidevoice-connector/actions/artifacts/{artifact_id}/zip`.
The run ID lives separately in provenance, and is verified through the API; it is not part of the download URL.
See [GitHub's artifact API](https://docs.github.com/en/rest/actions/artifacts).

Set `SIDEVOICE_GITHUB_TOKEN` to a token with **Actions: read on sidevoice/sidevoice-connector**. `GH_TOKEN` and
`GITHUB_TOKEN` are accepted as fallbacks. Do not commit credentials. The desktop repository's built-in Actions
token is not a cross-repository credential: trusted manual/main runs can use the repository secret
`SIDEVOICE_CONNECTOR_ACTIONS_READ`. It is deliberately withheld from PR code. PR builds of a ready pin need
appropriate read access or a trusted manual run; they must not silently substitute a fixture on auth failure.

The connector producer must emit this canonical URL and a SEA built against the genuine signed core manifest.
Connector `build.mjs` verifies the core manifest's Sigstore identity before embedding it; the installer verifies
core payloads before installation. Desktop preserves those checks and verifies that it packages the reviewed SEA.
Desktop independently verifies the SEA's Sigstore claim using the [official GitHub CLI verifier](https://cli.github.com/manual/gh_attestation_verify),
without duplicating the connector's core-manifest verifier. An ad-hoc macOS signature alone is not publisher
authentication. Do not substitute a synthetic manifest or a manifest-less PR SEA. The verifier needs network
access for trusted root material; this is not an offline build guarantee.

Actions artifacts and mutable nightly URLs are **temporary dogfood inputs**, not durable dependencies for a
versioned release. Expiry or nightly replacement fails closed. Obtain fresh genuine inputs and review a new pin;
do not remove verification or regenerate a digest from arbitrary downloaded bytes to make a build pass.

## Test boundaries

| Gate | Command / scope | Evidence |
| --- | --- | --- |
| Fast PR checks | `npm test`, `cargo test --locked --workspace`, fmt/clippy | Bridge, pin/transport, engine and local-host unit/contracts |
| Resource wiring while pin is pending | `npm run build:mac:fixture` | Explicit shell fixture, in `target/packages/fixture`; never a production artifact |
| Native integration | `npm run build:mac:probe`, `test/macos/*.sh` | WKWebView/local-host proxy, native engine, room model flow and card behavior |
| Product packaging | `npm run build:mac`, optional `npm run package:mac` | Real verified connector, production app and evidence JSON |
| Local R4 acceptance | Clean macOS arm64 account, real app | Install/start/pair/update-or-rollback results, recorded separately |

Native integration scripts run from the repository root on a **disposable macOS test account**. The engine/room
checks download speech models, and UI probes use the account's desktop and application state. They are not part of
the local packaging command. Actions gates the macOS jobs on fast tests; engine/card checks have separate path
filters so local-host transport changes do not always download and exercise speech models. Existing security and
WebView assertions remain. The native engine belongs to Desktop; moving those tests to Connector would test the
wrong product boundary.

Do not compile on the shared agent node. Use GitHub Actions for native builds. A manual run builds and uploads
artifacts without publishing; no merge or release is necessary to exercise this workflow. A manual run with
`fixture=true` explicitly checks a pending-pin fixture and native integration, and never uploads installers.
Leave it false for the genuine product gate; it cannot turn a missing signed dependency into a passing product build.

## Clean-account acceptance record

Use a new macOS arm64 account with no Sidevoice install. Record the exact Actions run/desktop SHA and retain
`build-evidence.json`, the app/DMG checksum, connector identity and web SHA. Launch the copied app via Finder or
`open`, following `MACOS.md`, so microphone permission belongs to the app.

Check the actual bundled interface: explicitly install the local host; observe ordered progress; verify the
service starts and the local host becomes reachable/paired; restart the app and verify the pairing remains.
Exercise an eligible update, or a controlled failed update/cancel and verify the previous working install remains
usable. Cancellation after commit must not kill the installer. Record each observed result and the installed
connector/core identities. A stand-in core, probe build, pending pin, or passing packaging check cannot satisfy
this record. Missing signed upstream assets or lack of a Mac must be reported as an unrun gate, not a pass.
