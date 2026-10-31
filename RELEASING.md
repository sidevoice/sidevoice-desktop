# Releasing

One version for the whole desktop app, tagged `vX.Y.Z`. It lives in `desktop/tauri.conf.json`, `package.json`,
`package-lock.json`, the three `Cargo.toml` and `Cargo.lock`; release-please moves them together
(`release-please-config.json`). Never edit them by hand. This repository will release a second product, the mobile
app, on its own: everything below is the desktop app's, and "Two products in one repository" says what keeps the
two apart.

## Two products in one repository

The mobile app (`mobile/`, plan: sidevoice/sidevoice-desktop#289) does not exist yet and nothing here builds it. What
is ready for it, so that adding it needs no change to the desktop app's tags, releases or nightly:

| | Desktop app (today) | Mobile app (when it exists) |
|---|---|---|
| release-please package | `.` (every commit; with the mobile package, every commit except the ones that touch only `mobile/`) | `mobile` |
| Tag | `vX.Y.Z` (unchanged) | `mobile-vX.Y.Z` |
| Changelog | `CHANGELOG.md` | `mobile/CHANGELOG.md` |
| Release PR | `chore(main): release X.Y.Z` | `chore(main): release mobile X.Y.Z` (its own PR and branch: `separate-pull-requests`) |
| Nightly | the `nightly` tag and pre-release, replaced by `build.yml` on a push to `main` that is not only `mobile/` | its own tag, from jobs of its own, outside the `needs` of the desktop's `publish-nightly` |
| CI cache keys | start with `desktop` (CI caches, below) | start with `mobile` |

- **The latest release is the desktop app's.** The releases' updater feed is
  `releases/latest/download/latest.json` (Updates, below): `latest` is one per repository. `xtask publish` therefore
  accepts only a desktop tag (`vX.Y.Z`, `-rc.N` allowed) and publishes nothing else, and a release of another product
  is never published as latest.
- **Adding the mobile package** is two edits of `release-please-config.json`, made together:
  - a block next to the `.` one, with its own `extra-files` (its `tauri.conf.json`, its `Cargo.toml` files, its
    `Cargo.lock` entries):
    `"mobile": { "release-type": "simple", "component": "mobile", "initial-version": "0.1.0", "bump-minor-pre-major": true,
    "extra-files": [ … ] }`. The tag takes the component by default (`include-component-in-tag` is `false` only on the
    desktop package);
  - `"exclude-paths": ["mobile"]` on the `.` package. release-please gives the root package every commit, so without it
    the mobile commits would land in the desktop's version and changelog.
- **`Release-As` once the mobile package exists.** `exclude-paths` also drops a commit that touches no file, and
  release-please gives such a commit to every package but the root. So the empty commit of "A release candidate, or any
  explicit version" (below) then sets the **mobile** version only; for the desktop app the footer goes on a commit that
  changes at least one file outside `mobile/` (the PR that needs the release does; with nothing else to merge, a
  one-line fix in this file). Until the mobile package exists there is no exclusion and the empty commit works for the
  desktop app as documented below.
- **A commit that changes only a shared crate** (`app-core/`, `app-tauri/`) belongs to the `.` package, not to `mobile`:
  release-please attributes a commit to a package by the files it touches, and a package is one directory. A mobile
  release for such a change needs a commit that touches `mobile/`, or the empty `Release-As` commit above.

## What each act means

| Act | Who | What happens |
|---|---|---|
| Open / update a PR | anyone | **Lint and unit tests** (Linux), **PR title is a conventional commit**, the **Windows** and **Linux** app builds (installers as artifacts), and the macOS native flows, every time: the probe app's (probe page, local host, call controls card) and a call on the engine's real models. No .dmg (a manual run of `build` on the branch makes one). |
| Squash-merge into `main` | reviewer | The PR title becomes the commit. `build` runs everything, installers included; when the installers' jobs (macOS app and .dmg, Linux, Windows) and the lint are green, the `nightly` pre-release is replaced. The macOS native flows run beside them: the nightly does not wait for them, and a failing one fails the run without holding the nightly back. release-please opens or updates the **release PR** ("chore(main): release X.Y.Z"). Nothing versioned is published. |
| Merge the release PR | a maintainer | **This is the release.** release-please tags `vX.Y.Z` and creates a draft GitHub Release whose notes are that version's changelog; `build` builds from the tag and, once every job passed (the macOS native flows too), attaches the assets and publishes the Release. |

Assets of a release: `Sidevoice_X.Y.Z_aarch64.dmg` (Apple Silicon, Developer ID signed and notarized, below),
`Sidevoice_X.Y.Z_x64-setup.exe`, `Sidevoice_X.Y.Z_amd64.deb`, `Sidevoice_X.Y.Z_amd64.AppImage`, `SHA256SUMS`, and
what the app updates itself from (Updates, below): `latest.json`, `Sidevoice_X.Y.Z_aarch64.app.tar.gz` and the
`.sig` of it, of the installer and of the AppImage.

The changelog is written from the squashed PR titles. To change it, edit `CHANGELOG.md` in the release PR right
before merging it: any later merge into `main` regenerates the PR. After the release, fix the notes on the
Release itself.

## The steps: `cargo xtask`

Everything besides setting up the machine and the GitHub wiring is code in `xtask/`, run as `cargo xtask <command>`
from the repository's root, the same on a runner and on a laptop. `xtask/src/main.rs` lists every command, what it
does and what it reads from the environment. The workflows only check out, install toolchains and caches, put the
secrets into the environment of the steps that need them, choose which steps run, and call:

- `test` job (Linux): `cargo xtask web` (the web interface at the pin), `icons`, `lint` (rustfmt and Clippy on
  the app and on xtask), `test` (the bridge script and the Rust tests), `two-users` (CI only: it adds a user); and
  xtask's own tests, `cargo test --manifest-path xtask/Cargo.toml`.
- `linux` and `windows` jobs: `cargo xtask bundle --channel C`, this host's installers.
- `macos` job (the app and .dmg; not on a pull request): `cargo xtask release app [--signed] --channel C`,
  `check-app [--signed]`, `first-open`, `package [--signed]`, `sign-package`, `checksums`, `forget-secrets`, `package-screens`.
  The only job with the Apple signing secrets.
- `macos-native` job, beside it: on the CI-only probe build, `probe-app`, `local-host`, `probe`, `card`, `exit`, `memory`. It shares
  the Rust build cache with `macos` (the same dependencies) and is the one that writes it (CI caches, below).
- `macos-call` job, beside both: `voice-models` and `voice-memory`, with the models they download cached by the
  engine's commit (`SIDEVOICE_REAL_CALL_MODELS`). `memory` and `voice-memory` measure the app's footprint and Metal
  allocation over a long session (#145) and fail only on clear growth: the table is in the step's log and summary.
- `publish-nightly` and `publish-release` jobs (the same steps): `cargo xtask publish assets release TAG` or
  `publish assets nightly`, then `publish release TAG` or `publish nightly`. The nightly waits for the lint and the
  installers' jobs only; a release waits for every job, `macos-native` and `macos-call` included.
- `.github/actions/setup-rust`: `cargo xtask engine-rev` and `sherpa-libs`; release-please.yml and pr-title.yml:
  `release-pr-checks` and `release-pr-title`.

On a laptop, `npm ci` first. The commands that start the app (`release first-open`, `probe probe`,
`probe card`) run it off a runner with a scratch home (`/tmp/svx-<step>`), never with this machine's own settings,
pairing, local core or models; `probe probe` expects no provider key in the login keychain, as on a runner. Unsigned and
probe builds ignore any `APPLE_*` variables in the environment. Every build is a `dev` build unless `--channel`
says otherwise (Channels, below), and only a `release` or `nightly` build reads the updater's key. Ctrl-C unwinds
cleanly; if a signing step was killed outright, `cargo xtask release forget-secrets` removes its keychain and key files.

## CI caches

The repository's Actions cache holds 10 GB, and one build's caches take about 4.5 GB. They are written only by
a push to `main`: the nightly's run, and a release's (release-please starts it from a push to `main` too). So they
belong to `main`, which every pull request reads. A pull request or a manual run, on any branch, only restores them:
writing its own would evict `main`'s and leave the next nightly to build from cold.

- The Rust build caches (`.github/actions/setup-rust`: `Swatinem/rust-cache` with `save-if`), one per job and keyed by
  `Cargo.lock`, except the one `macos` and `macos-native` share (`cache: macos`), which only `macos-native` writes
  (`save-cache: 'false'` on `macos`): it builds all of it. A pull request that changes `Cargo.lock` restores
  `main`'s closest one and builds what changed. There is one `Cargo.lock` for the repository (the root workspace), so
  a dependency of the mobile app changes the key of the desktop app's caches too: the next push to `main` writes them
  anew.
- sherpa-onnx's libraries (`setup-rust`, by the engine's commit) and the real call's models (`macos-call`, by the
  engine's commit and the test): `actions/cache/restore`, then `actions/cache/save` in a step that runs only on a
  push to `main`; the models' after the call passed.
- `publish-*`'s xtask cache: those jobs run only from a push to `main`.
- The npm and web-interface caches (a few tens of MB) are written by whichever run misses them. The web interface is
  one for both products (one `web.pin.json`), so its cache is too.
- **The keys of each product start with its name**: `setup-rust` is the desktop app's, and writes `v0-rust-desktop-…`
  and `sherpa-onnx-libs-desktop-…`; the mobile jobs will write `mobile`. A product's caches can then be listed, and
  one can never be taken for the other's (the sherpa libraries of an iOS build are not a macOS build's). All the
  products' caches share the same 10 GB: see the mobile jobs' PR for what they add.

## Channels

What a build is, decided when it is built: `--channel` of `cargo xtask release app` and `bundle`, which the workflow
sets from the run (`changes.channel`) and xtask hands to the app's build script (`SIDEVOICE_BUILD_CHANNEL`, with
`SIDEVOICE_BUILD_COMMIT` and `SIDEVOICE_BUILD_DATE`; `desktop/build.rs` refuses anything else). The app says it to
the page as `host.build` (docs/BRIDGE.md → "The build").

- `release`: a run called with a release's tag. Has the updater (`host.updater`, its checks) on the releases' feed
  and makes update artifacts (Updates, below). A release's macOS app is always `--signed`.
- `nightly`: a push to `main`. Built as its own version, `<the release-please manifest's version, patch + 1>-nightly.
  <GITHUB_RUN_NUMBER>` (after 0.4.2, run 17: `0.4.3-nightly.17`: above the last release, below the next, higher every
  run). Has the updater on the nightly's own feed and makes update artifacts (Updates, below); `host.build` gives that
  version, the short commit and that commit's date.
- `dev`: everything else, and the default: a laptop, a pull request, a manual run (a `sign` dry run too). No
  updater.

**A nightly's version on each platform.** The updater compares versions as semver: `X.Y.Z-nightly.N` sorts below
`X.Y.Z`, and `N` is compared as a number (`nightly.9` < `nightly.17`). A release in between moves the manifest, so
every nightly after it sorts above every nightly before it. The installers' own version fields take what each
platform allows:

- macOS: `CFBundleShortVersionString` is the whole version (what Finder and the app show). `CFBundleVersion` is at
  most three numbers, so a nightly's is `X.Y.Z` without `-nightly.N` (xtask sets `bundle.macOS.bundleVersion`): the
  same for every nightly until the next release. Nothing updates by it (the updater reads latest.json), and the
  notary takes both.
- Windows (NSIS): the installer's numeric `VIProductVersion` is `X.Y.Z.0` (the bundler drops the pre-release); the
  version it shows and registers is the whole one, which it compares as semver against an installed Sidevoice. No MSI
  is built: WiX takes no text in a pre-release, so a nightly MSI would need its own `bundle.windows.wix.version`.
- Linux: the AppImage carries the version in its name only. The .deb's `Version` is the whole one, which dpkg reads as
  `X.Y.Z` with the Debian revision `nightly.N`, above release `X.Y.Z`: installing exactly that release's .deb over the
  nightly is a downgrade to dpkg (it warns). A .deb never updates itself.

## Which version comes next

`fix:` → patch, `feat:` → minor. While the version is 0.x a breaking change (`feat!:` or a `BREAKING CHANGE:`
footer) bumps the minor, not the major. `docs:`, `chore:`, `ci:`, `test:`, `refactor:` alone make no release.

Nothing is tagged yet: the manifest starts at 0.1.0, so the first release PR proposes 0.2.0 if there is a `feat`.
To publish the first one as 0.1.0, use `Release-As: 0.1.0` (below).

## A release candidate, or any explicit version

Put the footer as the **last line of a PR's description** (the squash commit takes the description as its body):

```
Release-As: 0.3.0-rc.1
```

The release PR then proposes exactly that version. A version with a `-` suffix is published as a **pre-release
and never as latest**. The next candidate is `Release-As: 0.3.0-rc.2`; the final one is `Release-As: 0.3.0`
(say it: after a candidate, do not leave the next version to the computation). With nothing else to merge, a PR
with one empty commit (`git commit --allow-empty`) carries the footer. (Once the mobile package exists, an empty
commit versions the mobile app and the desktop app needs a commit that changes a file: "Two products in one
repository", above.)

## Nightly

Every green `build` on `main` (a push that touches only `mobile/` runs none) moves the tag `nightly` to that commit
and replaces every asset of the one `nightly` pre-release: `Sidevoice_nightly_aarch64.dmg`, `Sidevoice_nightly_x64-setup.exe`,
`Sidevoice_nightly_amd64.deb`, `Sidevoice_nightly_amd64.AppImage`, `SHA256SUMS`. Fixed names, so a link keeps
working, plus its updates and `latest.json`, the nightly's feed (Updates, below). Its notes give the commit and its
date. It is a snapshot, not a release: the app inside reports its nightly version (Channels, above), it is never
latest, and release-please ignores the tag (it is not `vX.Y.Z`). It is the desktop app's nightly; the mobile app's will
be a tag and a pre-release of its own.

Build artifacts on Actions runs are kept 7 days, for debugging only. Download from Releases.

## Updates

Each channel updates itself from its own feed (docs/BRIDGE.md → "The updater"), read at launch and then every hour, a
nightly's every 10 minutes, and only to a higher version than its own. The app chooses the feed and how often it reads
it by the channel it was built for (`desktop/core/src/updater.rs`, `feed` and `check_every`), so a build of one
channel is never offered the other's:

- A release reads the releases' feed,
  `https://github.com/sidevoice/sidevoice-desktop/releases/latest/download/latest.json`. `latest` is the newest
  release that is neither a draft nor a pre-release, so a `-rc.N` and the nightly are never offered.
- A nightly reads the nightly's own,
  `https://github.com/sidevoice/sidevoice-desktop/releases/download/nightly/latest.json`, which every green push to
  `main` replaces (Nightly, above): it updates itself to the next nightly, always a higher version (Channels, above).
- **A release ships while you run a nightly:** you stay on nightlies. A nightly never reads the releases' feed; the
  next nightly, built after the release from its version, is the one it updates to. To move to releases, install the
  release by hand (its .dmg, installer or AppImage): from then on that copy reads the releases' feed. A release becomes
  a nightly the same way, by installing the nightly. A release older than the nightly it replaces (`0.5.0` over
  `0.5.1-nightly.N`) is a downgrade: Windows' installer says a newer version is installed and offers to uninstall it
  first.

The app installs only what is signed with the updater's key, checked against the public key in
`desktop/tauri.conf.json` (`plugins.updater.pubkey`, minisign key id `EA1BE9B3A4CF715A`). The nightly's assets are
replaced one by one, so a check while that happens may miss one or pair one nightly's `latest.json` with another's
update, whose signature then does not match: nothing is installed, the check or the download fails
(`update-check-failed`, `update-download-failed`), and a later check finds them whole.

- **What a release (and a nightly) adds.** Only a `release` or `nightly` build makes them
  (`bundle.createUpdaterArtifacts`, passed by xtask as `--config`; such a build without the key fails before it
  starts):
  - macOS (`release app --signed --channel release`): `dist/Sidevoice_X.Y.Z_aarch64.app.tar.gz`, the signed, notarized
    and stapled app, checked again after unpacking (signature, ticket, Gatekeeper); a nightly's (`release app --signed
    --channel nightly`) likewise;
  - Windows and Linux (`bundle --channel release`): the NSIS installer and the AppImage themselves.

  Each comes with its `.sig`. `publish assets release TAG` checks all three and their signatures are among the
  artifacts, writes `latest.json` (`version` and `platforms` `darwin-aarch64`, `windows-x86_64`, `linux-x86_64`, each
  `{signature, url}` of that release's asset: all the updater reads) and adds them to the assets. `publish assets
  nightly` does the same for the nightly: its version, and the urls of its `Sidevoice_nightly_*` assets.
- **What cannot update itself.** A dev build (built without the updater), a `.deb` (the package manager
  owns it), an AppImage's binary run on its own, an app run from the .dmg: the page gets no `host.updater`.
- **The key.** The private key and its password are the repository secrets `TAURI_SIGNING_PRIVATE_KEY` and
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Only a release or nightly run resolves them (in any other they resolve to
  nothing), and xtask drops them from a dev build. Keep a copy outside GitHub: without the key no update can be
  signed for the apps out there, and a new key means a new public key in the app, which those apps do not have.
- **macOS and the signature.** An update replaces the app with one signed by the same Developer ID and team, so macOS
  keeps the microphone permission and the keychain's trust in the app. The nightly is signed the same way, so moving
  from it to a release keeps both too.

## macOS signing and notarization

A release's and the nightly's macOS app and .dmg are signed with the Developer ID and notarized (`build.yml`, job
`macos`), so Gatekeeper opens them on managed Macs too. Pull requests are ad-hoc signed (`docs/FIRST_OPEN.txt` ships beside their .dmg): their signing
steps are skipped, and every secret those steps name resolves to nothing unless the run signs. (A pull request
from a fork never has the repository's secrets.)

- The app: Tauri's bundler imports `APPLE_CERTIFICATE` into a keychain of its own, signs every binary with
  `APPLE_SIGNING_IDENTITY`, the hardened runtime and `desktop/packaging/macos/Entitlements.plist` (the microphone; the keychain
  items the app keeps need no entitlement outside the sandbox), notarizes it with the App Store Connect API key
  (`APPLE_API_KEY`, `APPLE_API_ISSUER`, and `APPLE_API_KEY_P8` written to a file only the job can read,
  `APPLE_API_KEY_PATH`) and staples the ticket.
- The .dmg: signed with the same identity from a keychain of the job's own, notarized with `notarytool` and stapled.
- Checked in the job: the Developer ID authority, the team, a secure timestamp, `stapler validate` and
  `spctl -a -vv` (`source=Notarized Developer ID`) on the app, on the .dmg, and on the app inside the .dmg.
- The step's own keychain, the certificate file and the key file are removed when the step ends, whatever
  happened, and `cargo xtask release forget-secrets` removes what a step cut short left at the end of the job. The
  bundler removes the keychain it made itself when it is done; one left by a bundler that failed
  half-way goes with the runner, a GitHub-hosted machine discarded after the job.

**A dry signed build**, nothing published: run `build` by hand with `sign` on (Actions → build → Run workflow, or
`gh workflow run build.yml -R sidevoice/sidevoice-desktop --ref main -f sign=true`). A manual run publishes nothing
(only a release or a push to `main` does); the signed .dmg is the run's `Sidevoice-macOS-AppleSilicon-dmg` artifact,
kept 7 days.

**Signing on a Mac**, the same steps: with the secrets above in the environment (on a laptop `APPLE_API_KEY_PATH`,
a private .p8 file, may stand for `APPLE_API_KEY_P8`, and without `APPLE_CERTIFICATE` the Developer ID in the login
keychain signs). The .p8 must belong to this user, have only one name, and be readable and writable by nobody else
(`chmod 600 "$APPLE_API_KEY_PATH"` on macOS). It is read through the shared private-file checks and copied into the
step's private scratch directory; cleanup removes the copy and preserves the original.

```sh
npm ci
cargo xtask release app --signed
cargo xtask release check-app --signed      # needs APPLE_TEAM_ID
cargo xtask release package --signed
cargo xtask release sign-package
cargo xtask release checksums
```

leaves the signed, notarized and stapled `dist/Sidevoice_X.Y.Z_aarch64.dmg` and its `dist/SHA256SUMS.txt`.

## When something fails

- The build of a release fails: the Release stays a draft, its tag in place. Fix forward if needed, then re-run
  the failed jobs of that `release-please` run (Actions). Nothing is published until every job passed.
- A `nightly` run fails: the previous snapshot stays. The next green push replaces it.

## What this needs from the repository settings

- Settings → Actions → General → **Allow GitHub Actions to create and approve pull requests**: without it
  release-please cannot open its PR (off as of 2026-10-01).
- Squash merging, with the PR title as the commit message.
- Repository secrets for macOS signing (a release, or a manual `sign` run): `APPLE_CERTIFICATE` (the Developer ID
  Application certificate and key, a base64 .p12), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`
  ("Developer ID Application: … (TEAMID)"), `APPLE_TEAM_ID`, and the notary's App Store Connect API key:
  `APPLE_API_KEY` (its id), `APPLE_API_ISSUER`, `APPLE_API_KEY_P8` (the .p8's contents). release-please.yml
  hands them to `build` (`secrets: inherit`).
- The mobile app's TestFlight upload (`.github/workflows/mobile-testflight.yml`, by hand from `main`) reads the App Store
  Connect key's four secrets above (`APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_P8`, `APPLE_TEAM_ID`) and needs the
  key to have the **Admin** role; the desktop notarization works with it too. Its job runs in the environment
  `testflight`, whose deployment branches should be `main` only (docs/MOBILE.md → "TestFlight").
- Repository secrets for the updates (a release): `TAURI_SIGNING_PRIVATE_KEY` (the minisign private key
  `tauri signer generate` wrote) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (Updates, above).
- Required check **PR title is a conventional commit**. release-please's own PR gets it through a dispatched run
  (its pushes start no workflow by themselves).

`cargo xtask release first-open` requires a fresh CI runner profile: it refuses to start if the
`ai.sidevoice.desktop` directory already exists in either Windows Known Folder (Roaming or Local AppData),
so saved settings and WebView2 storage cannot make the first-open check pass. Setting `CI=true` alone
does not bypass this check. Windows bundles made with `cargo xtask bundle` disable `GGML_NATIVE`
on laptops as well as CI so the installer does not target the build machine’s CPU.
