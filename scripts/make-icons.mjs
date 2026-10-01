#!/usr/bin/env node
// Draws everything the app shows of the Sidevoice mark, from the brand files in brand/ (scripts/vendor-brand.mjs):
//
//   src-tauri/icons/icon.icns             macOS app icon, every size (16…1024), on Apple's icon grid
//   src-tauri/icons/icon.ico              Windows app icon (16…256)
//   src-tauri/icons/{32x32,64x64,128x128,128x128@2x,icon}.png   Linux
//   src-tauri/icons/tray/{idle,live,muted}.png          macOS menu bar, template images (one ink)
//   src-tauri/icons/tray/{light,dark}-{idle,live,muted}.png   Windows / Linux tray, for light and dark bars
//   src-tauri/dmg/background.png, background@2x.png      the .dmg window
//   ui/brand/…                                           mark, lockups and DM Sans for the settings window
//
//   npm run icons
//
// The rules it follows are the brand's (the brand kit's README), and where they are silent, docs/BRAND.md.
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { Resvg } from "@resvg/resvg-js";
import { decodePng } from "./png.mjs";

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
const MARK_DARK = brand("logo/mark/sidevoice-mark-on-dark.svg"); // lila, mustard fourth bar (dark grounds)
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

// The bare mark on a square canvas of `size` px, its 24 box at 16 px (where every edge of the brand's grid
// lands on a whole pixel) and centred on whole pixels.
const bareMark = (size) => png(svg(size, size, place(MARK, (size - 16) / 2, (size - 16) / 2, 16)), size);

// Full bleed (Windows, Linux): the tile fills the square.
function appIconFlat(size) {
  if (size < minTile(1)) return bareMark(size);
  return png(svg(24, 24, inner(TILE)), size);
}

// macOS: Apple's icon grid on a 1024 canvas — an 824 body centred, with the system's soft drop shadow.
const MAC_BODY = 824 / 1024;
function appIconMac(size) {
  if (size < minTile(MAC_BODY)) return bareMark(size);
  const o = 100;
  const body = `<defs><filter id="s" x="-10%" y="-10%" width="120%" height="130%"><feGaussianBlur in="SourceAlpha" stdDeviation="10"/><feOffset dy="10"/><feComponentTransfer><feFuncA type="linear" slope="0.3"/></feComponentTransfer><feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge></filter></defs>` +
    `<g filter="url(#s)">${place(TILE, o, o, 1024 - 2 * o)}</g>`;
  return png(svg(1024, 1024, body), size);
}

// macOS does not read PNG data in the 16 and 32 px @1x slots reliably (icp4/icp5 drew as noise in the
// title bar of the .dmg window on CI), so those two use the classic formats every macOS reads: RGB in
// the icns run-length encoding (is32, il32) plus an 8-bit mask (s8mk, l8mk).
function packbits(bytes) {
  const out = [];
  for (let i = 0; i < bytes.length; ) {
    let run = 1;
    while (i + run < bytes.length && bytes[i + run] === bytes[i] && run < 130) run++;
    if (run >= 3) {
      out.push(0x80 + run - 3, bytes[i]);
      i += run;
      continue;
    }
    const start = i;
    while (i < bytes.length && i - start < 128 && !(i + 2 < bytes.length && bytes[i] === bytes[i + 1] && bytes[i] === bytes[i + 2])) i++;
    out.push(i - start - 1, ...bytes.subarray(start, i));
  }
  return Buffer.from(out);
}
function classic(pngData) {
  const { width, pixels } = decodePng(pngData);
  const n = width * width;
  const plane = (c) => Buffer.from(Array.from({ length: n }, (_, i) => pixels[i * 4 + c]));
  // The RGB is unpremultiplied (PNG), as the mask expects.
  return { rgb: Buffer.concat([0, 1, 2].map((c) => packbits(plane(c)))), mask: plane(3) };
}

// .icns: one entry per OSType; every size macOS asks for, 1x and 2x.
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
const [c16, c32] = [classic(mac[16]), classic(mac[32])];
out("src-tauri/icons/icon.icns", icns([
  ["is32", c16.rgb], ["s8mk", c16.mask], ["ic11", mac[32]], // 16, 16@2x
  ["il32", c32.rgb], ["l8mk", c32.mask], ["ic12", mac[64]], // 32, 32@2x
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
// mark's 24 box is drawn at 16 pt (32 px), where the brand's grid lands on whole pixels at 1x and at 2x,
// on whole-pixel offsets. Being in a call is a dot beside the mark, outside its clear space (a quarter of
// the bars' height: 5.25 units, 7 px), so the mark itself is never dimmed or redrawn.
//
//   px @2x:  6 | 32 mark box (bars 8…36) | 8 clear | 8 dot | 2   = 54 × 36 (27 × 18 pt)
const TRAY = { w: 54, h: 36, box: [6, 2, 32], dot: [48, 18, 4] };
const k = TRAY.box[2] / 24; // px per brand unit
const at = (u, axis) => TRAY.box[axis] + u * k;
const trayMark = (drawing) => place(drawing, ...TRAY.box);
const dot = (ink) => `<circle cx="${TRAY.dot[0]}" cy="${TRAY.dot[1]}" r="${TRAY.dot[2]}" fill="${ink}"/>`;

// Muted: a slash across the mark (top left to bottom right, as macOS's own microphone-off symbol), with a
// gap on each side so it reads at menu bar size. Box units: from (3, 3) to (21, 21), 2 wide, 1.5 of gap.
const slashLine = (width, colour, box = at) =>
  `<line x1="${box(3, 0)}" y1="${box(3, 1)}" x2="${box(21, 0)}" y2="${box(21, 1)}" stroke="${colour}" stroke-width="${width}" stroke-linecap="round"/>`;
const slashed = (drawing, ink, w, h, box = at, unit = k) =>
  `<defs><mask id="gap" maskUnits="userSpaceOnUse" x="0" y="0" width="${w}" height="${h}"><rect width="${w}" height="${h}" fill="#fff"/>${slashLine(5 * unit, "#000", box)}</mask></defs>` +
  `<g mask="url(#gap)">${drawing}</g>${slashLine(2 * unit, ink, box)}`;

// Template images (black + alpha; macOS tints them for light and dark menu bars and for highlight):
//   idle   no call — the one-ink mark
//   live   in a call, microphone open — the mark and the dot
//   muted  in a call, muted — the mark crossed out, and the dot
const template = {
  idle: trayMark(MARK_INK),
  live: trayMark(MARK_INK) + dot("#000"),
  muted: slashed(trayMark(MARK_INK), "#000", TRAY.w, TRAY.h) + dot("#000"),
};
for (const [state, body] of Object.entries(template)) {
  out(`src-tauri/icons/tray/${state}.png`, png(svg(TRAY.w, TRAY.h, body), TRAY.w));
}

// Windows and Linux do not tint tray icons and draw them square (16 px at 100 %), with their own spacing
// around each. The app picks the set for the bar's colour (src/tray.rs): berenjena on light, lila on dark.
// The bare mark at 16 px lands on whole pixels; the bitmap is 32 px, so it does at 2x too.
//   idle   the mark in one ink ("Tono": the fourth bar at 45 % of the ink)
//   live   the mark in colour: the other voice, mustard, is there
//   muted  the mark in colour, crossed out
const tono = (ink) => MARK_INK.replaceAll('fill="#000000"', `fill="${ink}"`);
const sq = (u, axis) => u * (32 / 24);
for (const [ground, colourMark, ink] of [["light", MARK, "#4a2f6b"], ["dark", MARK_DARK, "#c9b6ec"]]) {
  const states = {
    idle: place(tono(ink), 0, 0, 32),
    live: place(colourMark, 0, 0, 32),
    muted: slashed(place(colourMark, 0, 0, 32), ink, 32, 32, sq, 32 / 24),
  };
  for (const [state, body] of Object.entries(states)) {
    out(`src-tauri/icons/tray/${ground}-${state}.png`, png(svg(32, 32, body), 32));
  }
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
