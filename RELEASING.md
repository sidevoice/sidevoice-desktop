# Releasing

One version for the whole app, tagged `vX.Y.Z`. It lives in `src-tauri/tauri.conf.json`, `package.json`,
`package-lock.json`, the three `Cargo.toml` and `src-tauri/Cargo.lock`; release-please moves them together
(`release-please-config.json`). Never edit them by hand.

## What each act means

| Act | Who | What happens |
|---|---|---|
| Open / update a PR | anyone | **Lint and unit tests** (Linux) and **PR title is a conventional commit**. The macOS native flow (engine round trip, probe page, room flow) only when the engine, the bridge, the catalogue or the CI probes change. No installers. |
| Squash-merge into `main` | reviewer | The PR title becomes the commit. `build` runs everything, installers included; when it is green, the `nightly` pre-release is replaced. release-please opens or updates the **release PR** ("chore(main): release X.Y.Z"). Nothing versioned is published. |
| Merge the release PR | a maintainer | **This is the release.** release-please tags `vX.Y.Z` and creates a draft GitHub Release whose notes are that version's changelog; `build` builds from the tag, attaches the assets and publishes the Release. |

Assets of a release: `Sidevoice_X.Y.Z_aarch64.dmg` (Apple Silicon, ad-hoc signed: `docs/FIRST_OPEN.txt`),
`Sidevoice_X.Y.Z_x64-setup.exe`, `Sidevoice_X.Y.Z_amd64.deb`, `Sidevoice_X.Y.Z_amd64.AppImage`, `SHA256SUMS`.

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

## When something fails

- The build of a release fails: the Release stays a draft, its tag in place. Fix forward if needed, then re-run
  the failed jobs of that `release-please` run (Actions). Nothing is published until every job passed.
- A `nightly` run fails: the previous snapshot stays. The next green push replaces it.

## What this needs from the repository settings

- Settings → Actions → General → **Allow GitHub Actions to create and approve pull requests**: without it
  release-please cannot open its PR (off as of 2026-10-01).
- Squash merging, with the PR title as the commit message.
- Required check **PR title is a conventional commit**. release-please's own PR gets it through a dispatched run
  (its pushes start no workflow by themselves).
