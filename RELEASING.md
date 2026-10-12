# Releasing

One version for the whole app, tagged `vX.Y.Z`. It lives in `src-tauri/tauri.conf.json`, `package.json`,
`package-lock.json`, the three `Cargo.toml` and `src-tauri/Cargo.lock`; release-please moves them together
(`release-please-config.json`). Never edit them by hand.

## What each act means

| Act | Who | What happens |
|---|---|---|
| Open / update a PR | anyone | **Lint and unit tests** (Linux), **PR title is a conventional commit**, and the **Windows** and **Linux** app builds (installers as artifacts). The macOS native flow (probe page, a call on the engine's real models, call controls card) whenever native code (`src-tauri/`), the bridge, the web pin or the CI probes change; no .dmg (a manual run of `build` on the branch makes one). |
| Squash-merge into `main` | reviewer | The PR title becomes the commit. `build` runs everything, installers included; when it is green, the `nightly` pre-release is replaced. release-please opens or updates the **release PR** ("chore(main): release X.Y.Z"). Nothing versioned is published. |
| Merge the release PR | a maintainer | **This is the release.** release-please tags `vX.Y.Z` and creates a draft GitHub Release whose notes are that version's changelog; `build` builds from the tag, attaches the assets and publishes the Release. |

Assets of a release: `Sidevoice_X.Y.Z_aarch64.dmg` (Apple Silicon, Developer ID signed and notarized, below),
`Sidevoice_X.Y.Z_x64-setup.exe`, `Sidevoice_X.Y.Z_amd64.deb`, `Sidevoice_X.Y.Z_amd64.AppImage`, `SHA256SUMS`, and
what the app updates itself from (Updates, below): `latest.json`, `Sidevoice_X.Y.Z_aarch64.app.tar.gz` and the
`.sig` of it, of the installer and of the AppImage.

The changelog is written from the squashed PR titles. To change it, edit `CHANGELOG.md` in the release PR right
before merging it: any later merge into `main` regenerates the PR. After the release, fix the notes on the
Release itself.

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
with one empty commit (`git commit --allow-empty`) carries the footer.

## Nightly

Every green `build` on `main` moves the tag `nightly` to that commit and replaces every asset of the one
`nightly` pre-release: `Sidevoice_nightly_aarch64.dmg`, `Sidevoice_nightly_x64-setup.exe`,
`Sidevoice_nightly_amd64.deb`, `Sidevoice_nightly_amd64.AppImage`, `SHA256SUMS`. Fixed names, so a link keeps
working. Its notes give the commit and its date. It is a snapshot, not a version: the app inside reports the
version of the last release, it is never latest, and release-please ignores the tag (it is not `vX.Y.Z`).

Build artifacts on Actions runs are kept 7 days, for debugging only. Download from Releases.

## macOS signing and notarization

Only a release's macOS app and .dmg are signed with the Developer ID and notarized (`build.yml`, job `macos`).
Pull requests and the nightly are ad-hoc signed (`docs/FIRST_OPEN.txt` ships beside their .dmg): their signing
steps are skipped, and every secret those steps name resolves to nothing unless the run signs. (A pull request
from a fork never has the repository's secrets.)

- The app: Tauri's bundler imports `APPLE_CERTIFICATE` into a keychain of its own, signs every binary with
  `APPLE_SIGNING_IDENTITY`, the hardened runtime and `src-tauri/Entitlements.plist` (the microphone; the keychain
  items the app keeps need no entitlement outside the sandbox), notarizes it with the App Store Connect API key
  (`APPLE_API_KEY`, `APPLE_API_ISSUER`, and `APPLE_API_KEY_P8` written to a file only the job can read,
  `APPLE_API_KEY_PATH`) and staples the ticket.
- The .dmg: signed with the same identity from a keychain of the job's own, notarized with `notarytool` and stapled.
- Checked in the job: the Developer ID authority, the team, a secure timestamp, `stapler validate` and
  `spctl -a -vv` (`source=Notarized Developer ID`) on the app, on the .dmg, and on the app inside the .dmg.
- The job's own keychain, the certificate file and the key file are removed at the end of the job, whatever
  happened. The bundler removes the keychain it made itself when it is done; one left by a bundler that failed
  half-way goes with the runner, a GitHub-hosted machine discarded after the job.

**A dry signed build**, nothing published: run `build` by hand with `sign` on (Actions → build → Run workflow, or
`gh workflow run build.yml -R sidevoice/sidevoice-desktop --ref main -f sign=true`). A manual run publishes nothing
(only a release or a push to `main` does); the signed .dmg is the run's `Sidevoice-macOS-AppleSilicon-dmg` artifact,
kept 7 days.

## Updates

A running app updates itself from the releases (docs/BRIDGE.md → "The updater"): it reads
`https://github.com/sidevoice/sidevoice-desktop/releases/latest/download/latest.json` at launch and every 6 hours.
`latest` is the newest release that is neither a draft nor a pre-release, so a `-rc.N` and the nightly are never
offered. The app installs only what is signed with the updater's key, checked against the public key in
`src-tauri/tauri.conf.json` (`plugins.updater.pubkey`, minisign key id `EA1BE9B3A4CF715A`).

- **What a release adds.** Built only in the runs that sign, a release or a manual `sign` run (`changes.updates`):
  - macOS: `Sidevoice_X.Y.Z_aarch64.app.tar.gz`, the signed, notarized and stapled app (checked again after
    unpacking);
  - Windows: the NSIS installer itself;
  - Linux: the AppImage itself.

  Each comes with its `.sig`. The builds turn on `bundle.createUpdaterArtifacts` only then (`--config`): a build
  asked for them without the private key fails, and pull requests and the nightly never have it. The publish job of
  a release checks all three and their signatures, writes `latest.json` (`version`, `notes` (the release's notes),
  `pub_date`, and `platforms` `darwin-aarch64`, `windows-x86_64`, `linux-x86_64`, each `{signature, url}` of that
  release's asset), and attaches them to the release. The nightly gets none of it.
- **What cannot update itself.** A `.deb` (the package manager owns it), a dev build, an AppImage's binary run on its
  own: the page gets no `host.updater`. Nor can versions from before the first release with the updater: people on
  those reinstall once.
- **The key.** The private key and its password are the repository secrets `TAURI_SIGNING_PRIVATE_KEY` and
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. A step resolves them only in a run that builds update artifacts (in any
  other run they resolve to nothing). Keep a copy outside GitHub: without the key no update can be signed for the
  apps out there, and a new key means a new public key in the app, which those apps do not have. They would have to
  reinstall.
- **macOS and the signature.** An update replaces the app with one signed by the same Developer ID and team, so macOS
  keeps the microphone permission and the keychain's trust in the app. A nightly or CI build is ad-hoc signed: moving
  from one to a release may ask once more for both.
- **A dry run.** A manual `sign` run builds and signs the update artifacts too and publishes nothing: the macOS one is
  in its `Sidevoice-macOS-AppleSilicon-dmg` artifact, the AppImage's and the installer's `.sig` in
  `Sidevoice-Linux-x64` and `Sidevoice-Windows-x64`. `latest.json` is written only by a release's publish job.

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
- Repository secrets for the updates (a release, or a manual `sign` run): `TAURI_SIGNING_PRIVATE_KEY` (the minisign
  private key `tauri signer generate` wrote) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (Updates, above).
- Required check **PR title is a conventional commit**. release-please's own PR gets it through a dispatched run
  (its pushes start no workflow by themselves).
