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
`src-tauri/core/src/i18n.rs`.

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
- **One file per concern:** `platform/<concern>.rs`. When the behaviour differs per OS, that file declares a
  documented trait, and its `cfg` picks `platform/<concern>/<os>.rs`, each implementing the trait with the minimum
  code. The concern file holds only the trait, the shared types and that selection, never an OS body.
- **An OS that cannot do something still implements the method**, returning a documented fallback (`None`,
  "unknown", a coarser value) that callers handle. Callers never ask which OS they run on.
- **Only a value differs** (a folder name, a shortcut label): a plain module with that value is enough, no trait.
- **Callers import the trait and its type**; no forwarding wrappers or free functions repeating the trait.
- **Tests are written against the interface** and run on every OS; no per-OS test modules unless it cannot be
  phrased generically (say why in the PR).
- **Maintained crates first**; own `unsafe` only where nothing maintained exists, justified in the PR.
- **Enforced in CI**: new OS-specific code outside `platform` fails the build. Existing debt is listed in the
  repository's platform-debt.txt file, which may only shrink.

Reference: sidevoice-connector `connector/src/platform/user.rs` and `user/{unix,windows}.rs`.
