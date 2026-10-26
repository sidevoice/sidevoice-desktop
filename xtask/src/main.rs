//! Build tooling for the desktop app, run as `cargo xtask <command>` from the repository's root (alias in
//! `.cargo/config.toml`). Every step of CI is one of these: a workflow sets up the machine (toolchains, caches, the
//! secrets as environment variables) and calls them, and each runs the same on a person's machine. They call `npm`,
//! `node`, `cargo` (with the app's toolchain, src-tauri/rust-toolchain.toml), `git`, `gh` and the system's tools from
//! the `PATH`. RELEASING.md says which run calls which.
//!
//! Every pull request (Linux, the `test` job; `bundle` also on Windows):
//!
//! - `web`: the web interface at the pin (web.pin.json), into ui/voice/ (`npm run web`). Reads
//!   `SIDEVOICE_WEB_ATTESTATION` and `GH_TOKEN` (scripts/build-web.mjs).
//! - `platform-guard`: reject OS-specific code outside platform (the separate Platform boundary CI job).
//! - `platform-clippy`: deny OS API lints in the independent Platform boundary job.
//! - `lint`: rustfmt and Clippy (warnings denied) on the app's workspace and on this tooling.
//! - `test`: the bridge script's tests (`npm test`) and the app's Rust tests (`cargo test --workspace`).
//! - `icons`: `npm run icons`, then the icons, tray marks and .dmg window must be what is committed.
//! - `two-users` (Linux, CI only: it adds a user with `sudo`): the local host with two users; the second gets nowhere.
//! - `bundle [--channel C]` (Linux, Windows): this host's installers: the .deb and the AppImage, or the NSIS
//!   installer. With `--channel release` or `nightly`, the AppImage and the installer are also signed as updates
//!   (their `.sig`, checked).
//!
//! The Windows app, in the `windows` job after `bundle` (windows.rs):
//!
//! - `windows first-open` (a CI runner): the app started with no settings: the bundled interface comes up.
//!
//! Setting up a build (`.github/actions/setup-rust`):
//!
//! - `engine-rev`: the sidevoice-engine commit src-tauri/Cargo.lock pins, printed.
//! - `sherpa-libs`: sherpa-onnx's libraries, checked, by that engine's own `cargo xtask sherpa-libs`; prints their
//!   directory (`export SHERPA_ONNX_LIB_DIR="$(cargo xtask sherpa-libs)"`) and, on a runner, exports it.
//!
//! The macOS app and .dmg (Apple Silicon), in the order CI runs them (macos.rs, probe.rs): `app` to `dmg-screens` in
//! the `macos` job; beside it, `probe-app`, `local-host`, `probe`, `card` and `exit` in `macos-native`, and
//! `voice-models` and `voice-memory` in `macos-call`, `memory` after `exit`:
//!
//! - `macos app [--signed] [--channel C]`: the app bundle, ad-hoc signed; with `--signed`, signed with the Developer
//!   ID, notarized and stapled by Tauri's bundler. Reads `APPLE_SIGNING_IDENTITY`, `APPLE_API_KEY`,
//!   `APPLE_API_ISSUER`, and `APPLE_API_KEY_P8` (the .p8's contents, written to a 0600 file removed when it ends) or
//!   `APPLE_API_KEY_PATH` (a private .p8 on a laptop, checked and staged in the same scratch directory); `APPLE_CERTIFICATE` and `APPLE_CERTIFICATE_PASSWORD` when set (else
//!   the bundler signs from the login keychain). With `--signed --channel release` (or `--channel nightly`), also the
//!   update artifact: dist/Sidevoice_<version>_aarch64.app.tar.gz and its `.sig`, checked unpacked as the app is.
//! - `macos check-app [--signed]`: what a first open depends on: Info.plist, the signature (`--signed`: the Developer
//!   ID's, `APPLE_TEAM_ID`'s, timestamped, notarized, stapled, and Gatekeeper takes it), entitlements, the icon, arm64,
//!   no CI probe inside.
//! - `macos first-open`: the app started with no settings: the bundled interface comes up.
//! - `macos dmg [--signed]`: dist/Sidevoice_<version>_aarch64.dmg with the brand's window, verified, the app inside
//!   checked; without `--signed`, docs/FIRST_OPEN.txt beside it.
//! - `macos sign-dmg`: the .dmg signed with the Developer ID (from a keychain of its own holding
//!   `APPLE_CERTIFICATE`), notarized with notarytool, stapled, and checked with the app inside as a download has
//!   them. Reads what `macos app --signed` reads. Its keychain, certificate and key files are removed when it ends,
//!   whatever happened.
//! - `macos checksums`: dist/SHA256SUMS.txt for the .dmg.
//! - `macos forget-secrets`: whatever a signing step cut short left (its keychain, certificate and key files); CI
//!   runs it at the end of the job, whatever happened.
//! - `macos dmg-screens` (informational): the .dmg's window as Finder shows it, light and dark, into dmg-screens/.
//! - `macos probe-app`: the app built with the `probe` feature (CI only, never shipped) over the release app.
//! - `macos local-host`: on the probe app, the local host from the bundled page: pairing, the proxy, its refusals.
//! - `macos probe`: on the probe app, the probe page: origin, loopback, the native engine, the voice call, the headset.
//! - `macos voice-models`: a call on the engine's real models through the app's adapter (the ignored tests). Reads
//!   `SIDEVOICE_REAL_CALL_MODELS`: a folder to keep the models in between runs (CI caches it); unset, a scratch one.
//! - `macos voice-memory`: the host's calls on the engine's real models with the process's footprint and Metal allocation
//!   taken at each step (idle, a call with the models loaded, the call ended and the idle keep run out, ten calls, a
//!   call whose settings change), as a table; it fails only on clear growth (memory.rs says what and why).
//! - `macos memory`: on the probe app, the same over a long session in the app: models, calls, the call controls
//!   card's settings, the audio devices and the voice settings changed over and over (#145).
//! - `macos card`: on the probe app, the call controls card over a full-screen editor (screenshots into card-shots/).
//! - `macos exit`: on the probe app, quitting and restarting (as an installed update does) with a model on the GPU
//!   loaded: each process ends with status 0 and no crash report (#121).
//!
//! The commands that start the app (`first-open`, `probe`, `card`) run it with the runner's home on a
//! runner; off one (`CI` is not `true`), with a scratch home (`/tmp/svx-<step>`) and no `SIDEVOICE_DATA_DIR`, so
//! this machine's own settings, pairing, local core and models are never touched (`local-host` always has its own,
//! /tmp/svhome). `probe` expects no provider key in the login keychain, as on a runner. Unsigned and probe builds
//! drop every `APPLE_*` signing variable. Ctrl-C or SIGTERM stops the running program and unwinds: secret files,
//! keychains, mounts and background programs are removed; after a hard kill, `macos forget-secrets` removes what
//! signing left.
//!
//! Publishing (the `publish` job; publish.rs), `gh` with `GH_TOKEN` and `GH_REPO`:
//!
//! - `publish assets release TAG | nightly`: the installers among the build's artifacts (artifacts/) into assets/,
//!   under their published names (fixed `_nightly_` ones for the nightly), with SHA256SUMS; also every platform's
//!   update and its `.sig`, checked, and latest.json (a release's notes are release TAG's).
//! - `publish release TAG`: assets/ attached to the draft release TAG, which is then published (a `-rc.N` as a
//!   pre-release, never latest).
//! - `publish nightly`: the `nightly` tag moved to this commit (`GITHUB_SHA`, or HEAD) and the `nightly` pre-release
//!   replaced by assets/ (its notes say `WHEN` it was committed).
//!
//! The release PR (checks.rs), `gh` with `GH_TOKEN` and `GH_REPO`:
//!
//! - `release-pr-title [BRANCH]`: the PR on release-please's BRANCH (default `GITHUB_REF_NAME`) has a release title.
//! - `release-pr-checks`: the PR-title check dispatched on every open release PR's branch.
//!
//! Channels (RELEASING.md): `--channel release|nightly|dev` (default `dev`, so a laptop's build is never a release)
//! is what a Tauri build is told (`SIDEVOICE_BUILD_CHANNEL`, `_COMMIT`, `_DATE`; src-tauri/build.rs). A release
//! and a nightly build have the updater, each on its own channel's feed (the app chooses it by its channel), make
//! update artifacts and read `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (a dev build has
//! them removed); a nightly is built as `<manifest version, patch + 1>-nightly.<GITHUB_RUN_NUMBER>` (its macOS bundle
//! version without `-nightly.N`).
//!
//! This tooling's own tests: `cargo test --manifest-path xtask/Cargo.toml`.

mod checks;
mod macos;
mod memory;
mod platform_guard;
mod probe;
mod publish;
mod util;
mod windows;

use std::process::ExitCode;

use util::Channel;

pub type Result<T> = std::result::Result<T, String>;

const USAGE: &str =
    "usage: cargo xtask web | lint | platform-guard | platform-clippy | test | icons | two-users | bundle [--channel C] | engine-rev
  | sherpa-libs | macos app [--signed] [--channel C] | macos check-app [--signed] | macos first-open
  | macos dmg [--signed] | macos sign-dmg | macos checksums | macos forget-secrets | macos dmg-screens
  | macos probe-app | macos local-host | macos probe | macos voice-models | macos card | macos exit
  | macos memory | macos voice-memory
  | windows first-open
  | publish assets release TAG | publish assets nightly | publish release TAG | publish nightly
  | release-pr-title [BRANCH] | release-pr-checks
(C: release, nightly or dev, the default; xtask/src/main.rs says what each does)";

/// The `macos` commands run on macOS only: its tools (codesign, hdiutil, the window server) are the point.
fn macos(command: &str, step: impl FnOnce() -> Result<()>) -> Result<()> {
    util::require_os("macos", &format!("macos {command}"))?;
    step()
}

fn main() -> ExitCode {
    if let Err(error) = util::on_interrupt() {
        eprintln!("xtask: {error}");
        return ExitCode::FAILURE;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["web"] => checks::web(),
        ["lint"] => checks::lint(),
        ["platform-guard"] => platform_guard::check(),
        ["platform-clippy"] => checks::clippy(true),
        ["test"] => checks::test(),
        ["test-support"] => checks::test_support(),
        ["icons"] => checks::icons(),
        ["two-users"] => checks::two_users(),
        ["bundle"] => checks::bundle(Channel::Dev),
        ["bundle", "--channel", c] => Channel::parse(c).and_then(checks::bundle),
        ["engine-rev"] => checks::engine_rev(),
        ["sherpa-libs"] => checks::sherpa_libs(),
        ["macos", "app"] => macos("app", || macos::build_app(false, Channel::Dev)),
        ["macos", "app", "--signed"] => macos("app", || macos::build_app(true, Channel::Dev)),
        ["macos", "app", "--channel", c] => Channel::parse(c).and_then(|c| macos("app", || macos::build_app(false, c))),
        ["macos", "app", "--signed", "--channel", c] => {
            Channel::parse(c).and_then(|c| macos("app", || macos::build_app(true, c)))
        }
        ["macos", "check-app"] => macos("check-app", || macos::check_app(false)),
        ["macos", "check-app", "--signed"] => macos("check-app", || macos::check_app(true)),
        ["macos", "first-open"] => macos("first-open", macos::first_open),
        ["macos", "dmg"] => macos("dmg", || macos::dmg(false)),
        ["macos", "dmg", "--signed"] => macos("dmg", || macos::dmg(true)),
        ["macos", "sign-dmg"] => macos("sign-dmg", macos::sign_dmg),
        ["macos", "forget-secrets"] => macos("forget-secrets", macos::forget_secrets),
        ["macos", "checksums"] => macos("checksums", macos::checksums),
        ["macos", "dmg-screens"] => macos("dmg-screens", macos::dmg_screens),
        ["macos", "probe-app"] => macos("probe-app", probe::probe_app),
        ["macos", "local-host"] => macos("local-host", probe::local_host),
        ["macos", "probe"] => macos("probe", probe::probe),
        ["macos", "voice-models"] => macos("voice-models", probe::voice_models),
        ["macos", "card"] => macos("card", probe::card),
        ["macos", "exit"] => macos("exit", probe::exit),
        ["macos", "memory"] => macos("memory", probe::memory),
        ["macos", "voice-memory"] => macos("voice-memory", probe::voice_memory),
        ["windows", "first-open"] => {
            util::require_os("windows", "windows first-open").and_then(|()| windows::first_open())
        }
        ["publish", "assets", kind] => publish::assemble(kind, None),
        ["publish", "assets", kind, tag] => publish::assemble(kind, Some(tag)),
        ["publish", "release", tag] => publish::release(tag),
        ["publish", "nightly"] => publish::nightly(),
        ["release-pr-title"] => checks::release_pr_title(None),
        ["release-pr-title", branch] => checks::release_pr_title(Some(branch)),
        ["release-pr-checks"] => checks::release_pr_checks(),
        _ => Err(USAGE.into()),
    };
    if let Err(error) = &result {
        eprintln!("xtask: {error}");
    }
    ExitCode::from(u8::from(result.is_err()))
}
