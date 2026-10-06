<!-- Header: .github/assets/readme-header*.svg, from the Sidevoice brand's banner. Badges: shieldcn
     (https://shieldcn.dev), each a light/dark pair so the row follows the reader's GitHub theme. -->
<picture>
  <source media="(prefers-color-scheme: dark)" srcset=".github/assets/readme-header-on-dark.svg" />
  <img alt="Sidevoice — Give your coding agent a voice. Keep the conversation." src=".github/assets/readme-header.svg" width="750" />
</picture>

<p>
  <a href="https://github.com/sidevoice/sidevoice-desktop/actions/workflows/release.yml"><picture><source media="(prefers-color-scheme: dark)" srcset="https://shieldcn.dev/github/ci/sidevoice/sidevoice-desktop.svg?variant=secondary&size=sm&workflow=release.yml&branch=main&mode=dark" /><img alt="CI status" src="https://shieldcn.dev/github/ci/sidevoice/sidevoice-desktop.svg?variant=secondary&size=sm&workflow=release.yml&branch=main&mode=light" /></picture></a>
  <a href="LICENSE"><picture><source media="(prefers-color-scheme: dark)" srcset="https://shieldcn.dev/github/license/sidevoice/sidevoice-desktop.svg?variant=secondary&size=sm&mode=dark" /><img alt="licence" src="https://shieldcn.dev/github/license/sidevoice/sidevoice-desktop.svg?variant=secondary&size=sm&mode=light" /></picture></a>
  <picture><source media="(prefers-color-scheme: dark)" srcset="https://shieldcn.dev/badge/built_with-Tauri_2.svg?variant=secondary&size=sm&logo=tauri&mode=dark" /><img alt="built with Tauri 2" src="https://shieldcn.dev/badge/built_with-Tauri_2.svg?variant=secondary&size=sm&logo=tauri&mode=light" /></picture>
  <picture><source media="(prefers-color-scheme: dark)" srcset="https://shieldcn.dev/badge/status-beta.svg?variant=secondary&size=sm&mode=dark" /><img alt="status: beta" src="https://shieldcn.dev/badge/status-beta.svg?variant=secondary&size=sm&mode=light" /></picture>
</p>

# sidevoice-desktop

Reading your coding agent's plans, diffs and summaries all day is tiring. **Sidevoice** turns the conversation you
already have with your agent into a voice call. The agent keeps its context and keeps writing as usual; it also
speaks its replies, and you answer by voice and can interrupt it — from the sofa or on a walk, not only at your desk.

**sidevoice-desktop** is the app you call from, for macOS, Windows and Linux. A call lives in its own window with a
tray icon and a global mute shortcut, and on Apple-Silicon Macs speech models run natively on your computer.

## How it fits

| Piece | Role |
|---|---|
| [sidevoice-connector](https://github.com/sidevoice/sidevoice-connector) | What you install on the machine where your agents run: their voice tools, and the supervisor of that machine's core. |
| [sidevoice-core](https://github.com/sidevoice/sidevoice-core) | The conversations and the voice pipeline, next to the agents. The app talks to it. |
| **sidevoice-desktop** (this repository) | The app you call from. |
| [sidevoice-web](https://github.com/sidevoice/sidevoice-web) | The call interface. The app bundles a recorded build of it and never loads a remote page. |

## Status

Beta. What works today:

- Pairing with your machine through a one-time code, and calls with its conversations.
- The tray (menu-bar) icon with the call state (idle, live, muted): mute, hang up, show the window, settings, quit.
  Closing the window keeps the call going.
- A global mute shortcut (⌘⇧M on macOS, Ctrl+Shift+M elsewhere; configurable).
- On Apple-Silicon Macs (macOS 13+), transcription (Whisper) and speech (Kokoro) in the app's native engine,
  downloaded the first time you choose a model. On every platform, a provider (OpenAI, ElevenLabs) with your own
  key, kept on your machine. The Windows and Linux builds do not include a local speech engine yet.

The macOS build is ad-hoc signed and not notarized yet, so macOS blocks it on first open (below).

## Get it

From [**Releases**](https://github.com/sidevoice/sidevoice-desktop/releases). Until the first versioned release, use
the [`nightly`](https://github.com/sidevoice/sidevoice-desktop/releases/tag/nightly) pre-release, built from the
latest green `main`. Each carries the Apple-Silicon `.dmg`, the Windows installer, the Linux `.deb` and `.AppImage`, and
`SHA256SUMS`.

**First open on macOS.** Drag Sidevoice to Applications, then clear the quarantine flag once:

```sh
xattr -dr com.apple.quarantine /Applications/Sidevoice.app
```

(or open it once, then *System Settings → Privacy & Security → Open Anyway*). Start it from Finder, Spotlight or
`open /Applications/Sidevoice.app` — not by running the binary inside the bundle, or macOS gives the microphone
permission to your terminal — and allow the microphone when asked.

**Pair it.** On the machine where your agents run, with the
[connector](https://github.com/sidevoice/sidevoice-connector) installed, ask your agent to pair a device (or run
`npx @sidevoice/uplink pair-device`). Enter the one-time code in the app's settings, under machines. Then ask the
agent to join the voice call.

## Develop

You need Node.js 22, Rust (CI pins 1.98.1: `.github/actions/setup`) and Tauri's
[platform prerequisites](https://v2.tauri.app/start/prerequisites/) (on Linux: WebKitGTK 4.1, AppIndicator, librsvg,
OpenSSL and xdo development packages).

```sh
npm ci
npm test                                   # the bridge scripts
cargo test --locked                        # every crate (on Linux the shell needs WebKitGTK to build)
cargo clippy --locked --all-targets --all-features -- -D warnings
npx tauri dev                              # needs a desktop: macOS, Windows, or Linux with WebKitGTK 4.1
cargo xtask dist                           # this machine's installers, checked as a release ships them
cargo xtask smoke                          # macOS: the native flows CI runs (xtask/src/main.rs)
```

```
src-tauri/src/       the shell: windows, tray, shortcut, commands
src-tauri/core/      pure logic (settings, bridge contract, media rules), testable without Tauri
src-tauri/engine/    the native speech engine
src-tauri/local-host/ this computer's core: socket checks, local pairing, the page's proxy
xtask/               build tooling: installers, their checks, the native smoke flows, publishing
bridge/              the script injected into the call page: the page side of the bridge
ui/                  the settings window; ui/voice*/ hold the bundled web interface
brand/               the brand files the icons and installer art are generated from
docs/                the bridge, the engines and models, targets, macOS notes, the brand
```

The bundled interface is a build of [sidevoice-web](https://github.com/sidevoice/sidevoice-web), vendored with
`scripts/vendor-web.mjs`, which records the commit it came from in `ui/voice/web-source.json` ([`docs/TARGETS.md`](docs/TARGETS.md)). The app drives it only through an explicit bridge
([`docs/BRIDGE.md`](docs/BRIDGE.md)), never through its DOM.

## Contributing

Issues and pull requests are welcome. Read [`AGENTS.md`](AGENTS.md) first: it holds the rules for code, texts and
tests, for people and coding agents alike. Pull request titles follow
[Conventional Commits](https://www.conventionalcommits.org) (CI checks them) and become the squashed commit, from
which release notes are written ([`RELEASING.md`](RELEASING.md)).

## Licence

[Apache-2.0](LICENSE). The Sidevoice name and logo are trademarks: forks are welcome under their own name — see
[`TRADEMARKS.md`](TRADEMARKS.md).

## Third-party components

The app bundles third-party components under their own licences, among them eSpeak NG (GPL) inside the speech
workers of the bundled interface: see [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).
