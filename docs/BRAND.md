# The Sidevoice brand in the desktop app

The brand lives in [sidevoice/brand-resources](https://github.com/sidevoice/brand-resources) and its README
holds the rules. This app copies the files it draws from into `brand/` (`scripts/vendor-brand.mjs`, provenance
in `brand/source.json`) and generates everything else from them with `npm run icons` (`scripts/make-icons.mjs`).
CI fails if the committed icons are not what the script draws. `scripts/brand-screens.mjs` renders
`docs/brand-screens/` with headless Chromium, so the work can be looked at without a Mac.

Identity (chosen 2026-09-27): the "berenjena · Voz al lado" mark, mustard fourth bar, lila as the accent of
dark interfaces, and the one-ink variant "Tono" (the fourth bar at 45 % of the ink).

## What the app draws, and from which brand file

| Thing | Drawn from | Where |
|---|---|---|
| macOS app icon, every `.icns` entry (16 → 1024, 1x and 2x) | `logo/tile/sidevoice-tile.svg`; bare `logo/mark/sidevoice-mark.svg` at 16 px @1x | `src-tauri/icons/icon.icns` |
| Windows app icon (16, 20, 24, 32, 40, 48, 64, 256) | the tile; the bare mark (16 px box) at 16 and 20 px | `src-tauri/icons/icon.ico` |
| Linux icons (32, 64, 128, 256, 512) | the tile | `src-tauri/icons/*.png` |
| macOS menu bar: idle, live, muted | `logo/mark/sidevoice-mark-black.svg` (Tono) | `src-tauri/icons/tray/{idle,live,muted}.png` |
| Windows / Linux tray: idle, live, muted, for light and dark bars | `sidevoice-mark.svg`, `…-on-dark.svg`, and Tono in their inks | `src-tauri/icons/tray/{light,dark}-*.png` |
| `.dmg` window background (1x, 2x) | `logo/lockup/sidevoice-lockup.svg` | `src-tauri/dmg/`, laid out by `scripts/dmg-settings.py` |
| `.dmg` volume icon | the `.icns` above | CI |
| Settings window | lockups (light and dark), DM Sans, palette | `ui/brand/`, `ui/settings.css` |

The room window (pairing, calls, first open) is the web interface, bundled from rubasace/sidevoice
(`ui/voice/`, docs/TARGETS.md). Its brand is that repo's; it reaches the app the next time it is vendored.

## Rules followed as written

- **Colours**: berenjena on light grounds, lila on dark ones, mustard only for the fourth bar, noche on lila,
  white on berenjena, tinta for text on light. The settings window's light ground is bruma; its dark ground and
  surfaces are the room's (`#14171b`, `#1c2128`), so the two windows match.
- **One ink**: every monochrome drawing is the brand's Tono file, the fourth bar at 45 % of the ink. No state
  of the tray draws five bars in one solid colour.
- **Bars are never redrawn**: every drawing places the brand's own SVG, scaled uniformly.
- **Minimum size, 16 px for the mark**: the tile carries the mark at 72 %, so a tile smaller than 22.3 px
  (27.6 px on Apple's grid) would put the mark under 16 px. Those sizes use the bare mark instead, as the brand's
  own 16 px favicon does: `.icns` 16 px @1x, `.ico` 16 and 20 px (its 24 box at 16 px, centred on whole pixels,
  where every edge of the brand's grid lands on a pixel). Every other size is the tile.
- **Minimum size, 20 px of mark for a lockup**: the settings header draws the lockup's mark at 28 px, the
  `.dmg` window at 30 pt.
- **Clear space, a quarter of the mark's height**: the settings header leaves 24 px under the lockup (7 px
  needed) and the window's margins on the other sides; the `.dmg` lockup has 40 pt above and 42 pt below.
  In the menu bar, see below.
- **The wordmark is never retyped**: the lockups are the brand's outlined SVGs.
- **Typography**: DM Sans, the brand's Latin variable font, bundled in `ui/brand/` with its licence
  (`OFL.txt`). SIL OFL 1.1 allows bundling it with software, it declares no Reserved Font Name, and the licence
  travels with it. The settings window uses it for all text except code (the system monospace).

## Where the guidelines are silent: what was decided

1. **macOS icon grid.** The tile sits on Apple's 1024 grid (an 824 px body, centred) with the system's soft drop
   shadow (10 px down, 10 px blur, 30 % black). The tile keeps its own corner radius (6 of 24, 25 %), a
   little rounder than Apple's (≈ 22.4 %); the brand's tile is not reshaped. On macOS 26 the system may frame
   icons that are not built with Icon Composer: see "Left open".
2. **Windows and Linux icons are full bleed**: the tile fills the square, as icons there do.
3. **Tray states (macOS).** The menu bar image is a template (macOS tints it for light and dark bars, and when
   highlighted). The mark is always the Tono file at full ink; the state is drawn beside or over it:
   - *idle* (no call): the mark alone;
   - *live* (in a call, microphone open): the mark and a dot to its right, outside its clear space;
   - *muted*: the dot, and the mark crossed by a 2-unit slash with a 1.5-unit gap on each side, top left to
     bottom right (as macOS's own microphone-off symbol). The slash is a state drawn over the mark, not part of it.
   A first version dimmed the whole mark for *idle*; the review measured its fourth bar at 1.8 : 1 on a light bar,
   so the mark is never dimmed now.
4. **Tray size (macOS).** tray-icon draws a status item image 18 pt tall. The bitmap is 54 × 36 px (27 × 18 pt
   @2x). The mark's 24 box is 16 pt (32 px) at a 1 pt offset, so the brand's grid lands on whole pixels at 1x and
   2x (a first version at 18 pt blurred every bar edge). Clear space, a quarter of the bars' 21 units = 5.25
   units (3.5 pt): 4 pt left of the bars inside the image, 4 pt before the dot; above and below, 2 pt inside the
   image plus 3 pt of a 24 pt menu bar. Checked at 16, 18 and 22 pt on light and dark bars
   (`docs/brand-screens/tray@1x.png`, `@2x`).
5. **Windows / Linux tray.** Those systems do not tint tray icons and draw them square at 16 px (100 %), so it is
   the bare mark at a 16 px box (crisp; a 32 px bitmap for 2x) in the brand's look for the bar: berenjena on
   light, lila on dark. On Windows the app reads the taskbar's colour (`SystemUsesLightTheme`) at every icon
   change; Linux panels do not say, so they get lila (GNOME's top bar and most panels are dark). States: *idle*
   is the mark in one ink (Tono: the fourth bar at 45 % of berenjena or lila), *live* the mark in colour (the
   mustard voice appears), *muted* the mark in colour, crossed out. No dot: a square icon has no room outside
   the clear space. A first version used the tile, which blurred at 16 px (mark 11.5 px) — dropped.
6. **The bare mark at 16/20 px** (the small `.icns`/`.ico` entries) is the light look (berenjena), like the
   brand's `favicon-16.png`. A static icon cannot switch with the theme, so on a dark title bar or Finder list the
   berenjena bars read at 1.6 : 1 and only the mustard bar stands out; the tile instead would put the mark under
   16 px. **One of the two rules has to give at 16 px; which is the brand owner's call** (the review's point 5).
   The macOS 16 and 32 px @1x entries are stored as `is32`/`s8mk` and `il32`/`l8mk` (RLE RGB + mask), not PNG:
   the PNG slots (`icp4`/`icp5`) drew as noise in the .dmg window's title bar on CI.
7. **The `.dmg` window** (640 × 400 pt of picture; the window is 28 pt taller for Finder's title bar): bruma
   ground, the lockup centred on top, the app and Applications on a shelf with a white arrow between them. The
   icons' names are Finder's own text over the picture, and it is not certain it keeps them black in dark mode,
   so the shelf is a lilac that holds black and white text alike at 4.5 : 1 (`#7e6bab`: black 4.58, white 4.59).
   On CI (macOS 14) the names stayed black in both appearances. It is not a palette colour: it sits between berenjena and lila in the brand's hue, for this one
   purpose. The window is written by dmgbuild (no Finder scripting on CI); the background is one TIFF with the 1x
   and 2x drawings.
8. **Settings window spacing**: an 8 px rhythm (8/16/24/32), 10 px field radius, pill buttons as in the brand
   sheet's interface. Field borders are raised to 3 : 1 against the ground (WCAG 1.4.11): `#8c7eab` on light,
   `#6b6f7c` on dark. Focus rings and text selection use the accent.
9. **Product metadata**: name "Sidevoice"; short description "Give your agent a voice." (the brand sheet's
   card), long description from the brand's README banner. No copyright line: the holder of the desktop app's
   copyright is not written anywhere and is not guessed here.

## Left open

- **macOS 26 (Tahoe)** may draw legacy `.icns` icons inside a grey rounded square unless the app ships an Icon
  Composer asset (`Assets.car` with `CFBundleIconName`), which needs Xcode 26; the CI runner is macOS 14. Not
  checked on a real macOS 26.
- **Copyright / publisher** for the About panel and installers: the operator's to name.
- **NSIS installer artwork** (header and sidebar bitmaps): the installer uses the app icon only.
- The room window's brand fixes (rubasace/sidevoice `brand/web`) reach the app when it is vendored again.
