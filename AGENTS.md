# AGENTS.md

Rules for any coding agent (and person) working in this repository.

## Language of the code and of the product

- Code, identifiers, comments, commit messages and docs are in **English**.
- **Every user-facing text goes through i18n**: a key looked up in per-language message bundles (one file per
  language), with **English as the fallback** when a key is missing. No literal sentence in any language in
  components, server refusals and errors shown to people, native UI (tray, windows, notifications, installer or CLI
  output meant for people). Adding a text means adding its key to the English bundle; other languages may lag.
- Language defaults: the device's system language when we support it, otherwise English. Never Spanish, or any
  other language, as a hard-coded default.
- What reaches the agent (MCP instructions, tool results, the `[Sidevoice]` trailer) is English; it is not UI.

## Before changing things

Read `README.md` and `docs/`. The bundled web interface is the sidevoice-web release `web.pin.json` names (`scripts/build-web.mjs`): its texts
are fixed in sidevoice-web, not here. The app's own texts are the native ones (tray, …), in
`desktop/core/src/i18n.rs`.

## CI

- Logic lives in `cargo xtask` (`xtask/`), not in workflow YAML: a workflow sets up the machine and calls one
  `cargo xtask` command, which runs the same on a laptop (RELEASING.md, "The steps").

## Public product information

For changes to user-visible behavior, supported platforms/models, setup, security/privacy practices, availability, limitations, or release/download details, follow the shared [public-information process](https://github.com/sidevoice/landing/blob/main/AGENTS.md#keep-public-product-information-current). Record the landing change/PR or a linked `sidevoice/landing` issue in the PR checklist. Landing issues are the follow-up triage queue.

## Platform code

Operator rule (2026-10-25). Applies to product code, tests, xtask and build scripts.

- **Only `platform` touches the operating system.** Each crate has one `platform` module. It is the only place with
  `cfg(target_os | unix | windows | target_family)`, `cfg!` on those, `std::os::*`, `libc`, `rustix`, `windows-sys`,
  `objc2`/AppKit, OS commands, local sockets/pipes or OS-specific paths. Everything else is host-agnostic.
- **One file per concern:** `platform/<concern>.rs` exposes **public plain functions**; the common logic is written
  once there. Per-OS differences live in a **private trait named `Os`**, implemented by a type named `Native` (`impl Os for Native`)
  in `platform/<concern>/{macos,windows,linux}.rs`,
  with **one named method or const per real difference** and honest defaults (`None`, no-op, `false`,
  `Unsupported`). OS files override only what they actually do; an empty impl is fine. The concern file selects the
  OS file with `cfg_attr(target_os = ..., path = ...)` as `mod imp`, the only `cfg` allowed in common files; common
  code calls `Native::method()` / `Native::CONST` (`use imp::Native`). One exception: a
  value-only difference (no behaviour) may use `cfg!` in a plain module with no trait and no OS files, e.g. `data_dir`.
- **Platforms return observations/capabilities; decisions live in common code** (fallbacks, cascades, thresholds).
  No `fallback` fields, no closures or pieces passed by each OS into a common helper. An OS that cannot do
  something keeps the honest default; callers never ask which OS they run on (no caller OS gates).
- **What the UI must know is a const** (e.g. `SETTINGS_URL: Option<&str>`, `UNINSTALLABLE: bool`) read through a
  plain function.
- **Callers use the public functions** (`platform::<concern>::f()`); the trait and the OS types are private.
- **Libraries first:** use what Tauri and maintained crates already provide before writing platform code; own
  `unsafe` only where nothing maintained exists, justified in the PR.
- **No `allow(dead_code)`** to cover an OS that does not use something.
- **Tests are written against the interface** and run on every OS; no per-OS test modules unless it cannot be
  phrased generically (say why in the PR).
- **Enforced in CI** by its own `platform-boundary` check (next to Clippy's `disallowed-*`): any OS-specific code
  outside `platform` fails it. It stays red until the existing debt is fixed; there is no allowlist, with one exception: the crate attribute
  `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`, written exactly so as a file-scope inner attribute of a
  Cargo binary root (`src/main.rs`, `src/bin/*.rs`, `src/bin/*/main.rs`), which the linker reads from the crate root and so
  cannot live in `platform`.

Reference in this repo: `app-tauri/src/platform/microphone.rs`, `desktop/src/platform/desktop_geometry.rs` and `desktop/src/platform/confirmation.rs`, each public
plain functions over a private per-OS trait. (sidevoice-connector and core keep their separately approved public-trait
shape; do not copy it here.)

## Host kind is a crate boundary

Operator decision (2026-10-30). Applies like the platform rule above: product code, tests, xtask and build scripts.

- **A kind of host is a crate, never a condition.** The desktop app (`desktop/`) and the mobile app (`mobile/`) are
  separate crates at the workspace root, each composing the crates it uses. What both use lives in a shared crate
  (`app-core/`, `app-tauri/`) that takes whatever differs through a trait the app implements or a value it passes in,
  and never names an app. Not every crate exists yet: the checks look for each by its directory.
- **No `cfg(desktop)` or `cfg(mobile)`** (the conditions tauri-build defines), no `feature = "desktop"` or
  `feature = "mobile"`, and no `cfg_aliases!` entry named like one or built on one. Not outside `platform`, and not
  inside it. Where the OS is what differs, say which one: `target_os` in `platform`, one file per OS (`ios` and
  `android` are OSes like `macos`).
- **Dependencies point one way:** the apps name `app-tauri`, which names Tauri and `app-core`, which names voice and
  engine. `desktop/` and `mobile/` depend on neither each other nor anything that depends on the other, the shared
  crates on neither app, and `app-core` has no shell (Tauri, wry, tao) in its closure.
- **A build for iOS or Android names its package:** `-p`, never `--workspace`, `--all` or `--exclude`; the workspace
  holds the other kind of host too.
- **Enforced in CI**, in the Platform boundary job: `cargo xtask platform-guard` (the conditions, in every Rust file,
  `platform` included, and the features, target tables and lints of every `Cargo.toml`) and `cargo xtask host-graph`
  (the crates' dependencies on each supported target with every feature on, from `cargo metadata`, and the steps of the
  workflows that build for a mobile target). They read tokens, manifests, workflows and resolution and build nothing;
  their limits are in the head of `xtask/src/platform_guard.rs` and `xtask/src/host_graph.rs`.
