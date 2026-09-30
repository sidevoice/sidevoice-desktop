#!/usr/bin/env node
// Draws everything the app shows of the Sidevoice mark, from the brand files in brand/ (scripts/vendor-brand.mjs):
//
//   src-tauri/icons/icon.icns             macOS app icon, every size (16…1024), on Apple's icon grid
//   src-tauri/icons/icon.ico              Windows app icon (16…256)
//   src-tauri/icons/{32x32,64x64,128x128,128x128@2x,icon}.png   Linux
//   src-tauri/icons/tray/{idle,live,muted}.png          macOS menu bar, template images (one ink)
//   src-tauri/icons/tray/color-{idle,live,muted}.png    Windows / Linux tray, in colour
//   src-tauri/dmg/background.png, background@2x.png      the .dmg window
//   ui/brand/…                                           mark, lockups and DM Sans for the settings window
//
//   npm run icons
//
// The rules it follows are the brand's (brand-resources README), and where they are silent, docs/BRAND.md.
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { Resvg } from "@resvg/resvg-js";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const brand = (file) => readFileSync(path.join(root, "brand", file), "utf8");
const out = (file, data) => {
  mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
  writeFileSync(path.join(root, file), data);
};

// ------------------------------------------------------------------ the brand drawings

// A brand SVG's drawing (what is inside <svg>), on its own 24-unit box.
const inner = (svg) => svg.replace(/^<svg[^>]*>|<\/svg>\s*$/g, "");
const MARK = brand("logo/mark/sidevoice-mark.svg"); // berenjena, mustard fourth bar (light grounds)
const MARK_INK = brand("logo/mark/sidevoice-mark-black.svg"); // one ink, "Tono": the fourth bar at 45 %
const TILE = brand("logo/tile/sidevoice-tile.svg"); // the app icon: lila + mustard on berenjena
const LOCKUP = brand("logo/lockup/sidevoice-lockup.svg");
const box = (svg) => svg.match(/viewBox="([^"]+)"/)[1].split(" ").map(Number);

// The mark's bars, as the brand draws them: heights 6:12:21:12:6 on a 24 box.
const BARS = [...MARK.matchAll(/<rect x="([\d.]+)" y="([\d.]+)" width="([\d.]+)" height="([\d.]+)"/g)].map((m) => m.slice(1).map(Number));
const MARK_HEIGHT = Math.max(...BARS.map(([, , , h]) => h)); // 21 of 24
const TILE_PLATE = TILE.match(/<rect width="24" height="24" rx="([\d.]+)"/);
if (BARS.length !== 5 || MARK_HEIGHT !== 21 || !TILE_PLATE) throw new Error("brand/ is not the mark this script knows");
// The tile holds the mark at 72 %: its bars span 4.44…19.56 of 24.
const TILE_MARK = 0.72;

const svg = (w, h, body, viewBox = `0 0 ${w} ${h}`) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="${viewBox}">${body}</svg>`;
const place = (drawing, x, y, size, attrs = "") =>
  `<svg x="${x}" y="${y}" width="${size}" height="${size}" viewBox="0 0 24 24"${attrs}>${inner(drawing)}</svg>`;

function png(svgText, width) {
  return new Resvg(svgText, { fitTo: { mode: "width", value: width }, font: { loadSystemFonts: false } }).render().asPng();
}

// ------------------------------------------------------------------ app icons

// Below this size the tile would carry the mark under the brand's 16 px minimum (the tile draws it at
// 72 %), so the icon is the bare mark instead, as the brand's own 16 px favicon is.
const minTile = (fraction) => 16 / (TILE_MARK * fraction);

// Full bleed (Windows, Linux): the tile fills the square.
function appIconFlat(size) {
  if (size < minTile(1)) return png(svg(24, 24, inner(MARK)), size);
  return png(svg(24, 24, inner(TILE)), size);
}

// macOS: Apple's icon grid on a 1024 canvas — an 824 body centred, with the system's soft drop shadow.
const MAC_BODY = 824 / 1024;
function appIconMac(size) {
  if (size < minTile(MAC_BODY)) return png(svg(24, 24, inner(MARK)), size);
  const o = 100;
  const body = `<defs><filter id="s" x="-10%" y="-10%" width="120%" height="130%"><feGaussianBlur in="SourceAlpha" stdDeviation="10"/><feOffset dy="10"/><feComponentTransfer><feFuncA type="linear" slope="0.3"/></feComponentTransfer><feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge></filter></defs>` +
    `<g filter="url(#s)">${place(TILE, o, o, 1024 - 2 * o)}</g>`;
  return png(svg(1024, 1024, body), size);
}

// .icns: PNG entries by OSType; every size macOS asks for, 1x and 2x.
function icns(entries) {
  const chunks = entries.map(([type, data]) => {
    const head = Buffer.alloc(8);
    head.write(type, 0, "ascii");
    head.writeUInt32BE(8 + data.length, 4);
    return Buffer.concat([head, data]);
  });
  const head = Buffer.alloc(8);
  head.write("icns", 0, "ascii");
  head.writeUInt32BE(8 + chunks.reduce((n, c) => n + c.length, 0), 4);
  return Buffer.concat([head, ...chunks]);
}

// .ico: PNG entries (Windows Vista and later read them at every size).
function ico(images) {
  const head = Buffer.alloc(6);
  head.writeUInt16LE(0, 0);
  head.writeUInt16LE(1, 2);
  head.writeUInt16LE(images.length, 4);
  let offset = 6 + 16 * images.length;
  const dir = images.map(([size, data]) => {
    const e = Buffer.alloc(16);
    e.writeUInt8(size >= 256 ? 0 : size, 0);
    e.writeUInt8(size >= 256 ? 0 : size, 1);
    e.writeUInt16LE(1, 4); // planes
    e.writeUInt16LE(32, 6); // bits per pixel
    e.writeUInt32LE(data.length, 8);
    e.writeUInt32LE(offset, 12);
    offset += data.length;
    return e;
  });
  return Buffer.concat([head, ...dir, ...images.map(([, data]) => data)]);
}

const mac = Object.fromEntries([16, 32, 64, 128, 256, 512, 1024].map((s) => [s, appIconMac(s)]));
out("src-tauri/icons/icon.icns", icns([
  ["icp4", mac[16]], ["ic11", mac[32]], // 16, 16@2x
  ["icp5", mac[32]], ["ic12", mac[64]], // 32, 32@2x
  ["ic07", mac[128]], ["ic13", mac[256]], // 128, 128@2x
  ["ic08", mac[256]], ["ic14", mac[512]], // 256, 256@2x
  ["ic09", mac[512]], ["ic10", mac[1024]], // 512, 512@2x
]));
out("src-tauri/icons/icon.ico", ico([16, 20, 24, 32, 40, 48, 64, 256].map((s) => [s, appIconFlat(s)])));
for (const [file, size] of [["32x32.png", 32], ["64x64.png", 64], ["128x128.png", 128], ["128x128@2x.png", 256], ["icon.png", 512]]) {
  out(`src-tauri/icons/${file}`, appIconFlat(size));
}
// For the screens in docs/brand-screens and anyone who wants to look at it.
out("src-tauri/icons/macos-1024.png", mac[1024]);

// ------------------------------------------------------------------ tray

// macOS draws a status item image 18 pt tall (tray-icon), so the bitmap is 36 px tall for Retina. The
// mark's 24 box fills that height: 18 pt of mark, above the 16 px minimum. Its clear space (a quarter of
// the bars' 21-unit height, 5.25 units) comes from the menu bar above and below, and is drawn into the
// image left and right: 32 × 24 units.
const TRAY_W = 32;
const TRAY_H = 24;
const TRAY_PX = 36;
const trayMark = (drawing, attrs = "") => place(drawing, (TRAY_W - 24) / 2, 0, 24, attrs);

// Muted: a slash across the mark, with a gap on each side so it reads at menu bar size.
const SLASH = { x1: 7, y1: 3, x2: 25, y2: 21 };
const slashLine = (width, colour) =>
  `<line x1="${SLASH.x1}" y1="${SLASH.y1}" x2="${SLASH.x2}" y2="${SLASH.y2}" stroke="${colour}" stroke-width="${width}" stroke-linecap="round"/>`;
const slashed = (drawing, ink) =>
  `<defs><mask id="gap" maskUnits="userSpaceOnUse" x="0" y="0" width="${TRAY_W}" height="${TRAY_H}"><rect width="${TRAY_W}" height="${TRAY_H}" fill="#fff"/>${slashLine(5, "#000")}</mask></defs>` +
  `<g mask="url(#gap)">${drawing}</g>${slashLine(2, ink)}`;

// Template images (black + alpha; macOS tints them for light and dark menu bars and for highlight):
//   idle   no call — the one-ink mark, quieter (the whole ink at 55 %, the fourth bar still 45 % of it)
//   live   in a call, microphone open — the one-ink mark at full ink
//   muted  in a call, muted — the one-ink mark at full ink, crossed out
const IDLE_INK = 0.55;
const template = {
  idle: `<g opacity="${IDLE_INK}">${trayMark(MARK_INK)}</g>`,
  live: trayMark(MARK_INK),
  muted: slashed(trayMark(MARK_INK), "#000"),
};
for (const [state, body] of Object.entries(template)) {
  out(`src-tauri/icons/tray/${state}.png`, png(svg(TRAY_W, TRAY_H, body), (TRAY_PX * TRAY_W) / TRAY_H));
}

// Windows and Linux do not tint tray icons, and their bars can be light or dark: the tile reads on
// both. Same states: idle quieter, live as drawn, muted crossed out (lila slash, like the bars).
const colourTile = place(TILE, 0, 0, 24);
const colour = {
  idle: `<g opacity="${IDLE_INK}">${colourTile}</g>`,
  live: colourTile,
  muted: `<defs><mask id="gap" maskUnits="userSpaceOnUse" x="0" y="0" width="24" height="24"><rect width="24" height="24" fill="#fff"/><line x1="5" y1="5" x2="19" y2="19" stroke="#000" stroke-width="4.5" stroke-linecap="round"/></mask></defs>` +
    `${place(TILE, 0, 0, 24).replace(/(<rect x=[^>]*\/>)+/, (bars) => `<g mask="url(#gap)">${bars}</g>`)}` +
    `<line x1="5" y1="5" x2="19" y2="19" stroke="#c9b6ec" stroke-width="1.8" stroke-linecap="round"/>`,
};
for (const [state, body] of Object.entries(colour)) {
  out(`src-tauri/icons/tray/color-${state}.png`, png(svg(24, 24, body), 64));
}

// ------------------------------------------------------------------ .dmg window

// 640 × 400 pt, drawn at 1x and 2x (CI joins them into one HiDPI TIFF). The app sits left, Applications
// right (scripts/dmg-settings.py places them at these centres), an arrow between; the lockup on top.
// Finder writes the icons' names in black in light mode and in white in dark mode, over whatever the
// background is, so the icons stand on a shelf in a lilac that holds both at 4.5 : 1 (docs/BRAND.md).
const DMG = { width: 640, height: 400, app: [170, 214], applications: [470, 214] };
const SHELF = "#7e6bab"; // black text 4.58 : 1, white text 4.59 : 1
const [lw, lh] = box(LOCKUP).slice(2);
const LOCKUP_H = 30; // the mark's box at 30 pt: above the lockup's 20 px minimum at 1x
const lockupW = (lw * LOCKUP_H) / lh;
const arrow = (() => {
  const y = DMG.app[1];
  const [x1, x2] = [DMG.app[0] + 92, DMG.applications[0] - 92];
  return `<path d="M${x1} ${y}H${x2}M${x2 - 16} ${y - 16}L${x2} ${y}L${x2 - 16} ${y + 16}" fill="none" stroke="#ffffff" stroke-opacity="0.9" stroke-width="5" stroke-linecap="round" stroke-linejoin="round"/>`;
})();
const dmg = svg(DMG.width, DMG.height,
  `<rect width="${DMG.width}" height="${DMG.height}" fill="#f3eefa"/>` +
  `<svg x="${(DMG.width - lockupW) / 2}" y="40" width="${lockupW}" height="${LOCKUP_H}" viewBox="0 0 ${lw} ${lh}">${inner(LOCKUP)}</svg>` +
  `<rect x="32" y="112" width="${DMG.width - 64}" height="236" rx="24" fill="${SHELF}"/>` +
  arrow);
out("src-tauri/dmg/background.png", png(dmg, DMG.width));
out("src-tauri/dmg/background@2x.png", png(dmg, DMG.width * 2));
writeFileSync(path.join(root, "src-tauri/dmg/layout.json"), JSON.stringify(DMG, null, 2) + "\n");

// ------------------------------------------------------------------ the settings window's brand files

for (const file of [
  "logo/mark/sidevoice-mark.svg",
  "logo/mark/sidevoice-mark-on-dark.svg",
  "logo/lockup/sidevoice-lockup.svg",
  "logo/lockup/sidevoice-lockup-on-dark.svg",
  "fonts/DMSans-Variable-latin.woff2",
  "fonts/OFL.txt",
]) {
  mkdirSync(path.join(root, "ui/brand"), { recursive: true });
  copyFileSync(path.join(root, "brand", file), path.join(root, "ui/brand", path.basename(file)));
}
console.log("icons, tray, .dmg background and ui/brand written from", JSON.parse(brand("source.json")).commit.slice(0, 7));
