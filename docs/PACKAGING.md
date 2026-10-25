# Packaging layout

`src-tauri/tauri.conf.json` owns common product metadata, version, build, security and the updater public key.
Tauri automatically applies `tauri.macos.conf.json`, `tauri.windows.conf.json` or `tauri.linux.conf.json`, then
xtask's dev/release/nightly overrides. [Tauri uses JSON Merge Patch](https://v2.tauri.app/develop/configuration-files/#platform-specific-configuration):
objects merge and arrays replace, so each OS overlay contains the complete target and icon arrays.

| OS | Bundle targets | Resources |
| --- | --- | --- |
| macOS | `app`, `dmg` | `packaging/macos/`: plists, ICNS, icon preview, DMG backgrounds and layout |
| Windows | `nsis` | `packaging/windows/`: ICO |
| Linux | `deb`, `appimage` | `packaging/linux/icons/`: PNGs |
| Shared runtime | — | `packaging/shared/tray/`: tray PNGs included by Rust |

Paths are relative to `src-tauri/`. The full original icon order is retained in every overlay, including the PNG
fallbacks used for the default window icon; relocating an icon does not change which icon Tauri selects.
`npm run icons` regenerates the same bytes at the new paths. `cargo xtask icons` checks those directories.
The generator, brand previews, Rust `include_bytes!`, xtask's custom DMG build and documentation use this layout.

All three overlays retain `app.macOSPrivateApi: true`: Cargo currently enables Tauri's `macos-private-api`
feature globally. Tauri's build script requires the config flag to match even on Linux/Windows when invoked
directly through Cargo (including CI's Clippy/tests). Moving that Cargo feature to a target-specific dependency
would change more than the packaging layout. The flag has no native effect outside macOS.

## P26 migration evidence

Baseline: `02916bf3484be5f01e9225f9f17ed1d68ae54f20` (main before this change).

```sh
cargo xtask packaging compare 02916bf3484be5f01e9225f9f17ed1d68ae54f20
```

The comparison prints the full effective configuration for every OS × dev/release/nightly combination and fails
on any difference. It normalizes the relocated resource paths, projects the old combined target list onto the
formats supported by each OS, and omits macOS-only settings on Windows/Linux. The existing channel override is
applied last on both sides, including the nightly version, numeric macOS bundle version and updater artifacts.
It also compares the Git blob hash of every relocated resource against the baseline, including tray images,
plists and DMG assets. All nine configurations and all 22 resource hashes match.

The existing PR build jobs record sorted bundle file lists with `cargo xtask packaging files DIR OUTPUT` and
upload them as `packaging-linux`, `packaging-windows` and `packaging-macos-probe`. These include staged package
contents, files, directories and symlinks; the command never follows symlinks. Normal non-PR builds also record
`packaging-macos` for the shipped app. Installer artifact upload paths and job conditions are unchanged. No
additional macOS installer build is enabled for PRs.

The last successful baseline build is [40144091528](https://github.com/sidevoice/sidevoice-desktop/actions/runs/40144091528)
at `57ed4f2cf2b66a23bcde10ea2bb8d61aadfce72e`; the following main run's macOS installer job failed before this
change. Its existing `Sidevoice-*` artifacts provide the before installer file lists. Build timestamps, signatures
and executables are not expected to be byte-identical across builds; the resource hashes and effective config
comparison establish the layout-only change.

Windows installer policy/artwork and Linux package metadata remain follow-up work (P27/P28). This migration adds
no installer behavior, signing choices, hooks, resources or package formats.
