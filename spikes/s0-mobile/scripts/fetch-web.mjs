#!/usr/bin/env node
// S0 spike: builds dist/ — the launcher (launcher/) at its root and, unchanged, the web release the desktop app pins
// (the repository, tag and SHA-256 below, copied from web.pin.json at the time of the spike; this directory reads no
// file of the desktop app). The release tarball and SHA256SUMS are downloaded, checked against the pin, unpacked, and
// every directory of the site (voice/ among them) copied to dist/.
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const PIN = {
  repository: "sidevoice/sidevoice-web",
  tag: "v0.14.0",
  sha256: "7f6455e900cc8aa0d5f73c03db792c07b6909c161f78158100b6daa8c5bbb384",
};

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const work = path.join(root, "target", "web", PIN.sha256);
const site = path.join(work, "site");
const asset = `sidevoice-web-${PIN.tag.slice(1)}.tar.gz`;

if (!existsSync(path.join(work, ".done"))) {
  rmSync(work, { recursive: true, force: true });
  mkdirSync(site, { recursive: true });
  const base = `https://github.com/${PIN.repository}/releases/download/${PIN.tag}`;
  const download = async (name) => {
    const res = await fetch(`${base}/${name}`);
    if (!res.ok) throw new Error(`${PIN.repository} ${PIN.tag}: ${name} answered ${res.status}`);
    return new Uint8Array(await res.arrayBuffer());
  };
  const bytes = await download(asset);
  const digest = createHash("sha256").update(bytes).digest("hex");
  if (digest !== PIN.sha256) throw new Error(`${asset} is ${digest}, the pin says ${PIN.sha256}`);
  const sums = new TextDecoder().decode(await download("SHA256SUMS"));
  const listed = sums.split("\n").map((line) => line.trim().split(/\s+\*?/)).find(([, name]) => name === asset)?.[0];
  if (listed !== PIN.sha256) throw new Error(`SHA256SUMS lists ${asset} as ${listed ?? "nothing"}`);
  const tarball = path.join(work, asset);
  writeFileSync(tarball, bytes);
  execFileSync("tar", ["-xzf", tarball, "-C", site], { stdio: "inherit" });
  writeFileSync(path.join(work, ".done"), "");
}

const dist = path.join(root, "dist");
rmSync(dist, { recursive: true, force: true });
mkdirSync(dist, { recursive: true });
const served = readdirSync(site, { withFileTypes: true }).filter((e) => e.isDirectory()).map((e) => e.name);
if (!served.includes("voice")) throw new Error(`${PIN.tag} has no voice/ page`);
for (const name of served) cpSync(path.join(site, name), path.join(dist, name), { recursive: true });
cpSync(path.join(root, "launcher"), dist, { recursive: true });
writeFileSync(path.join(dist, "voice", "web-source.json"), JSON.stringify(PIN, null, 2) + "\n");
console.log(`dist/: launcher + ${PIN.repository} ${PIN.tag} (${served.map((n) => `${n}/`).join(", ")})`);
