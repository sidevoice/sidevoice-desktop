// The platform overlays (desktop/tauri/tauri.<os>.conf.json, docs/PACKAGING.md) give every place a person sees an icon the
// brand's (docs/BRAND.md): the Windows installer and uninstaller as well as the app, the macOS app at every size, and Linux a
// hicolor icon at every size it asks for. Without these keys the bundlers fall back to their own (NSIS's installer disc).
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const tauri = path.join(path.dirname(fileURLToPath(import.meta.url)), "../desktop/tauri");
const overlay = (os) => JSON.parse(readFileSync(path.join(tauri, `tauri.${os}.conf.json`), "utf8"));
const read = (icon) => readFileSync(path.join(tauri, icon));
const ascending = (sizes) => sizes.toSorted((a, b) => a - b);

// An .ico's image sizes, from its directory (a width of 0 stands for 256).
function icoSizes(data) {
  assert.equal(data.readUInt16LE(2), 1, "not an icon file");
  return Array.from({ length: data.readUInt16LE(4) }, (_, i) => data[6 + 16 * i] || 256);
}

// A PNG's width and height, from its IHDR.
function pngSize(data) {
  assert.equal(data.toString("latin1", 1, 4), "PNG", "not a PNG");
  return [data.readUInt32BE(16), data.readUInt32BE(20)];
}

// An .icns's entry types.
function icnsTypes(data) {
  assert.equal(data.toString("latin1", 0, 4), "icns", "not an icns file");
  const types = [];
  for (let at = 8; at < data.length; at += data.readUInt32BE(at + 4)) types.push(data.toString("latin1", at, at + 4));
  return types;
}

test("every overlay names the same icons, and each one exists", () => {
  // Arrays replace in Tauri's JSON Merge Patch, so each overlay holds the whole list.
  const [linux, ...others] = ["linux", "macos", "windows"].map((os) => overlay(os).bundle.icon);
  for (const icons of others) assert.deepEqual(icons, linux);
  for (const icon of linux) assert.ok(existsSync(path.join(tauri, icon)), `${icon} does not exist`);
});

test("the Windows installer and uninstaller carry the app's icon, not NSIS's", () => {
  const { bundle } = overlay("windows");
  const ico = bundle.icon.find((icon) => icon.endsWith(".ico"));
  assert.equal(bundle.windows?.nsis?.installerIcon, ico, "bundle.windows.nsis.installerIcon");
  assert.equal(bundle.windows?.nsis?.uninstallerIcon, ico, "bundle.windows.nsis.uninstallerIcon");
  assert.deepEqual(ascending(icoSizes(read(ico))), [16, 20, 24, 32, 40, 48, 64, 256]);
});

test("the macOS app icon has every size macOS asks for, 1x and 2x", () => {
  const icns = overlay("macos").bundle.icon.find((icon) => icon.endsWith(".icns"));
  // 16 and 32 @1x as RGB + mask (docs/BRAND.md); 16@2x, 32@2x, 128, 128@2x, 256, 256@2x, 512, 512@2x as PNG.
  const want = ["is32", "s8mk", "il32", "l8mk", "ic11", "ic12", "ic07", "ic13", "ic08", "ic14", "ic09", "ic10"];
  assert.deepEqual(icnsTypes(read(icns)).toSorted(), want.toSorted());
});

test("Linux gets a hicolor icon at every size, each filed under its own size", () => {
  const pngs = overlay("linux").bundle.icon.filter((icon) => icon.endsWith(".png"));
  const sizes = pngs.map((icon) => {
    // The bundler files a PNG under hicolor/<width>x<height>/apps, and under a `@2` directory when its name has `@2x`:
    // the icon theme reads that directory as twice its size in pixels, so such a PNG would be filed one size off.
    assert.doesNotMatch(icon, /@2x/, `${icon}: name it by its plain size`);
    const [width, height] = pngSize(read(icon));
    assert.equal(width, height, `${icon} is not square`);
    assert.equal(path.basename(icon), width === 512 ? "icon.png" : `${width}x${width}.png`, `${icon} is ${width} px`);
    return width;
  });
  // 48 is the size the freedesktop icon theme spec asks every application for; 16 and 24 are panel and menu sizes.
  assert.deepEqual(ascending(sizes), [16, 24, 32, 48, 64, 128, 256, 512]);
  // tauri-codegen makes the first PNG the window's own icon on Linux: it stays the 32 px one it has always been.
  assert.deepEqual(pngSize(read(pngs[0])), [32, 32]);
});
