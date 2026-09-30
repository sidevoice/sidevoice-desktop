#!/usr/bin/env node
// Renders docs/brand-screens/: the icons as each platform shows them (read back out of the .icns and .ico,
// so the containers are checked too), the tray on light and dark bars, the .dmg window, and the settings
// window in light and dark, with headless Chromium. Not part of the build: it is how the brand work is
// looked at without a Mac.
//
//   CHROME=/path/to/chromium [CHROME_LIBS=/extra/libs] node scripts/brand-screens.mjs
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync, cpSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { encodePng } from "./png.mjs";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const shots = path.join(root, "docs/brand-screens");
const work = mkdtempSync(path.join(os.tmpdir(), "brand-screens-"));
mkdirSync(shots, { recursive: true });

const CHROME = process.env.CHROME;
if (!CHROME || !existsSync(CHROME)) {
  console.error("set CHROME to a Chromium binary");
  process.exit(2);
}
function shot(name, page, width, height, scale = 1) {
  const file = path.join(shots, name);
  for (let i = 0; i < 3; i++) {
    rmSync(file, { force: true });
    try {
      execFileSync(CHROME, [
        "--headless", "--no-sandbox", "--disable-gpu", "--hide-scrollbars", "--allow-file-access-from-files",
        `--force-device-scale-factor=${scale}`, `--window-size=${width},${height}`, "--virtual-time-budget=5000",
        `--screenshot=${file}`, `file://${page}`,
      ], { stdio: "ignore", env: { ...process.env, LD_LIBRARY_PATH: process.env.CHROME_LIBS || "" } });
    } catch {}
    if (existsSync(file)) return console.log("wrote", path.relative(root, file));
  }
  throw new Error(`could not render ${name}`);
}

// ------------------------------------------------------------------ read the containers back

function readIcns(buf) {
  if (buf.toString("ascii", 0, 4) !== "icns" || buf.readUInt32BE(4) !== buf.length) throw new Error("bad icns");
  const entries = [];
  for (let at = 8; at < buf.length; ) {
    const len = buf.readUInt32BE(at + 4);
    entries.push([buf.toString("ascii", at, at + 4), buf.subarray(at + 8, at + len)]);
    at += len;
  }
  return entries;
}
function readIco(buf) {
  const n = buf.readUInt16LE(4);
  return Array.from({ length: n }, (_, i) => {
    const e = 6 + 16 * i;
    const size = buf.readUInt8(e) || 256;
    return [size, buf.subarray(buf.readUInt32LE(e + 12), buf.readUInt32LE(e + 12) + buf.readUInt32LE(e + 8))];
  });
}
const pngSize = (data) => [data.readUInt32BE(16), data.readUInt32BE(20)];
const ICNS_POINTS = { is32: "16 pt @1x", ic11: "16 pt @2x", il32: "32 pt @1x", ic12: "32 pt @2x", ic07: "128 pt @1x", ic13: "128 pt @2x", ic08: "256 pt @1x", ic14: "256 pt @2x", ic09: "512 pt @1x", ic10: "512 pt @2x" };

// The classic 16/32 px entries (RLE RGB + 8-bit mask) are decoded back into PNGs to be shown like the rest.
function unpackbits(data, count) {
  const out = [];
  for (let i = 0; out.length < count; ) {
    const n = data[i++];
    if (n >= 0x80) out.push(...Array(n - 0x80 + 3).fill(data[i++]));
    else for (let j = 0; j <= n; j++) out.push(data[i++]);
  }
  return out;
}
function classicToPng(rgb, mask, width) {
  const n = width * width;
  const planes = unpackbits(rgb, 3 * n);
  const pixels = Buffer.alloc(4 * n);
  for (let i = 0; i < n; i++) pixels.set([planes[i], planes[n + i], planes[2 * n + i], mask[i]], 4 * i);
  return encodePng({ width, height: width, pixels });
}
const rawIcns = new Map(readIcns(readFileSync(path.join(root, "src-tauri/icons/icon.icns"))));
const icns = [...rawIcns].filter(([type]) => !["s8mk", "l8mk"].includes(type)).map(([type, data]) =>
  type === "is32" ? [type, classicToPng(data, rawIcns.get("s8mk"), 16)]
  : type === "il32" ? [type, classicToPng(data, rawIcns.get("l8mk"), 32)]
  : [type, data]);
const ico = readIco(readFileSync(path.join(root, "src-tauri/icons/icon.ico")));
const img = (name, data) => {
  writeFileSync(path.join(work, name), data);
  return name;
};
const icnsCells = icns.map(([type, data]) => {
  const [w] = pngSize(data);
  const pt = ICNS_POINTS[type];
  const shown = Math.min(w, 128);
  return `<figure><div class="ground"><img src="${img(`icns-${type}.png`, data)}" width="${shown}" height="${shown}"></div><figcaption>${type} · ${w} px · ${pt}</figcaption></figure>`;
}).join("");
const icoCells = ico.map(([size, data]) =>
  `<figure><div class="pair"><div class="light"><img src="${img(`ico-${size}.png`, data)}" width="${size}" height="${size}"></div><div class="dark"><img src="ico-${size}.png" width="${size}" height="${size}"></div></div><figcaption>${size} px</figcaption></figure>`).join("");

// Small sizes shown 1:1 and magnified, pixel for pixel.
const small = icns.filter(([, d]) => pngSize(d)[0] <= 64).map(([type, data]) => {
  const [w] = pngSize(data);
  return `<figure><div class="ground"><img class="px" src="icns-${type}.png" width="${w * 4}" height="${w * 4}"></div><figcaption>${type} · ${w} px ×4</figcaption></figure>`;
}).join("");

for (const f of ["idle", "live", "muted", "light-idle", "light-live", "light-muted", "dark-idle", "dark-live", "dark-muted"]) {
  cpSync(path.join(root, `src-tauri/icons/tray/${f}.png`), path.join(work, `tray-${f}.png`));
}
cpSync(path.join(root, "src-tauri/dmg/background@2x.png"), path.join(work, "dmg@2x.png"));
cpSync(path.join(root, "src-tauri/icons/macos-1024.png"), path.join(work, "macos-1024.png"));
const layout = JSON.parse(readFileSync(path.join(root, "src-tauri/dmg/layout.json"), "utf8"));

// A menu bar: template images are drawn in the bar's text colour (macOS does the same with their alpha).
const bar = (dark, heightPt) => {
  const ink = dark ? "#ffffffe6" : "#000000d9";
  const items = ["idle", "live", "muted"].map((s) => {
    const w = (heightPt * 54) / 36;
    return `<span class="tpl" style="width:${w}px;height:${heightPt}px;background:${ink};-webkit-mask-image:url(tray-${s}.png)"></span>`;
  }).join("");
  return `<div class="menubar ${dark ? "dark" : "light"}"><span class="clock">idle · live · muted</span>${items}<span class="clock">14:05</span></div>`;
};
const taskbar = (dark, size) => `<div class="taskbar ${dark ? "dark" : "light"}">${["idle", "live", "muted"].map((s) => `<img src="tray-${dark ? "dark" : "light"}-${s}.png" width="${size}" height="${size}">`).join("")}<span>${size} px</span></div>`;

const css = `
@font-face { font-family: "DM Sans"; src: url("${path.join(root, "ui/brand/DMSans-Variable-latin.woff2")}"); font-weight: 100 1000; }
body { margin: 0; padding: 24px; font: 13px/1.4 "DM Sans", sans-serif; color: #1b1d24; background: #e9e4dc; }
h2 { font-size: 13px; letter-spacing: .08em; text-transform: uppercase; margin: 22px 0 10px; color: #5b5670; }
.row { display: flex; flex-wrap: wrap; gap: 14px; align-items: flex-end; }
figure { margin: 0; display: flex; flex-direction: column; align-items: center; gap: 6px; }
figcaption { font-size: 11px; color: #5b5670; }
.ground { background: #fff; padding: 10px; border-radius: 10px; display: grid; place-items: center; }
.pair { display: flex; }
.pair > div { padding: 10px; display: grid; place-items: center; }
.light { background: #f3f3f3; } .dark { background: #202020; }
.px { image-rendering: pixelated; }
.menubar { display: flex; align-items: center; gap: 10px; height: 24px; padding: 0 12px; border-radius: 6px; font: 13px -apple-system, "DM Sans", sans-serif; }
.menubar.light { background: #ececec; color: #000000d9; } .menubar.dark { background: #2b2b2b; color: #ffffffe6; }
.menubar .clock { font-size: 12px; opacity: .8; }
.tpl { display: inline-block; -webkit-mask-size: 100% 100%; }
.taskbar { display: flex; align-items: center; gap: 10px; height: 40px; padding: 0 12px; border-radius: 6px; font-size: 11px; }
.taskbar.light { background: #f3f3f3; color: #333; } .taskbar.dark { background: #1c1c1c; color: #ccc; }
.stack { display: flex; flex-direction: column; gap: 8px; }
`;

writeFileSync(path.join(work, "icons.html"), `<!doctype html><meta charset="utf-8"><style>${css}</style>
<h2>macOS · icon.icns (every entry, read back from the file)</h2><div class="row">${icnsCells}</div>
<h2>macOS · the small entries, pixel for pixel</h2><div class="row">${small}</div>
<h2>Windows · icon.ico on light and dark</h2><div class="row">${icoCells}</div>
`);
shot("icons@2x.png", path.join(work, "icons.html"), 1180, 1000, 2);
shot("icons@1x.png", path.join(work, "icons.html"), 1180, 1000, 1);

writeFileSync(path.join(work, "tray.html"), `<!doctype html><meta charset="utf-8"><style>${css}</style>
<h2>Menu bar (macOS template images, as macOS tints them) · 18 pt as drawn, and 16 / 22 pt</h2>
<div class="row"><div class="stack">${bar(false, 18)}${bar(true, 18)}</div><div class="stack">${bar(false, 16)}${bar(true, 16)}</div><div class="stack">${bar(false, 22)}${bar(true, 22)}</div></div>
<h2>Windows / Linux tray · berenjena on light bars, lila on dark · idle, live, muted</h2>
<div class="row"><div class="stack">${taskbar(false, 16)}${taskbar(true, 16)}</div><div class="stack">${taskbar(false, 24)}${taskbar(true, 24)}</div><div class="stack">${taskbar(false, 32)}${taskbar(true, 32)}</div></div>
`);
shot("tray@2x.png", path.join(work, "tray.html"), 900, 330, 2);
shot("tray@1x.png", path.join(work, "tray.html"), 900, 330, 1);

// The .dmg window as Finder lays it out: background, the app icon and a stand-in for Applications at the
// centres scripts/dmg-settings.py uses, with Finder's labels in both appearances.
const label = (x, y, text, dark) => `<div style="position:absolute;left:${x - 70}px;top:${y + 68}px;width:140px;text-align:center;font:12px -apple-system,sans-serif;color:${dark ? "#fff" : "#000"}">${text}</div>`;
const dmgPage = (dark) => `<!doctype html><meta charset="utf-8"><style>body{margin:0}</style>
<div style="position:relative;width:${layout.width}px;height:${layout.height}px;background:url(dmg@2x.png) 0 0/100% 100%">
<img src="macos-1024.png" width="128" height="128" style="position:absolute;left:${layout.app[0] - 64}px;top:${layout.app[1] - 64}px">
<div style="position:absolute;left:${layout.applications[0] - 56}px;top:${layout.applications[1] - 46}px;width:112px;height:92px;border-radius:10px;background:#5aa9f5"></div>
${label(...layout.app, "Sidevoice.app", dark)}${label(...layout.applications, "Applications", dark)}</div>`;
writeFileSync(path.join(work, "dmg-light.html"), dmgPage(false));
writeFileSync(path.join(work, "dmg-dark.html"), dmgPage(true));
shot("dmg-window-light-labels.png", path.join(work, "dmg-light.html"), layout.width, layout.height, 2);
shot("dmg-window-dark-labels.png", path.join(work, "dmg-dark.html"), layout.width, layout.height, 2);

// The settings window, in light and dark, with the app's commands answered by a stub.
const stub = `<script>window.__TAURI_INTERNALS__={invoke:async(c)=>{
  if(c==="get_settings")return{settings:{target:"",muteShortcut:"CmdOrCtrl+Shift+M"},debug:false,appVersion:"0.1.0"};
  if(c==="headset_report")return{platformSupported:true,muteGestureApi:true,inCall:true,testing:false,
    events:[{at:Date.now()-42000,source:"remote:toggle",action:"mute"},{at:Date.now()-31000,source:"airpods:unmute",action:"unmute"}]};
  return {};}};</script>`;
for (const scheme of ["light", "dark"]) {
  const html = readFileSync(path.join(root, "ui/index.html"), "utf8")
    .replace("<head>", `<head><base href="file://${path.join(root, "ui")}/">`)
    .replace('<link rel="stylesheet"', `${stub}<meta name="color-scheme" content="${scheme}"><style>:root{color-scheme:${scheme}}</style><link rel="stylesheet"`)
    .replace(/<meta http-equiv="Content-Security-Policy"[^>]*>/, "");
  // prefers-color-scheme follows the flag below; the page is otherwise the one the app ships.
  const page = path.join(work, `settings-${scheme}.html`);
  writeFileSync(page, html);
  const file = path.join(shots, `settings-${scheme}.png`);
  rmSync(file, { force: true });
  for (let i = 0; i < 3 && !existsSync(file); i++) {
    try {
      execFileSync(CHROME, [
        "--headless", "--no-sandbox", "--disable-gpu", "--hide-scrollbars", "--allow-file-access-from-files",
        "--force-device-scale-factor=2", "--window-size=600,900", "--virtual-time-budget=5000",
        `--blink-settings=preferredColorScheme=${scheme === "dark" ? 0 : 1}`, `--screenshot=${file}`, `file://${page}`,
      ], { stdio: "ignore", env: { ...process.env, LD_LIBRARY_PATH: process.env.CHROME_LIBS || "" } });
    } catch {}
  }
  console.log("wrote", path.relative(root, file));
}
rmSync(work, { recursive: true, force: true });
