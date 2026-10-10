// A released Sidevoice web site, as sidevoice-web's release workflow publishes it (its RELEASING.md): per release, the
// static site as `sidevoice-web-<version>.tar.gz` (`sidevoice-web-nightly.tar.gz` for `nightly`), `SHA256SUMS`, and
// an attestation over the tarball (`attestation.sigstore.json`). `web.pin.json` names the release and the tarball's
// SHA-256; `fetchSite` downloads it, checks it, and unpacks it. Nothing is built here.
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";

/**
 * The pin, checked: a GitHub repository (owner/name), a release tag, and the full SHA-256 of its tarball.
 * @returns {{ repository: string, tag: string, sha256: string }}
 */
export function readPin(pin) {
  const { repository, tag, sha256 } = pin ?? {};
  if (!/^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/.test(repository ?? "")) throw new Error("web.pin.json: `repository` must be owner/name");
  if (!/^(nightly|v\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?)$/.test(tag ?? "")) throw new Error("web.pin.json: `tag` must be a vX.Y.Z release or nightly");
  if (!/^[0-9a-f]{64}$/.test(sha256 ?? "")) throw new Error("web.pin.json: `sha256` must be the tarball's full SHA-256 (64 hex)");
  return { repository, tag, sha256 };
}

/** The tarball a release carries: `sidevoice-web-<version>.tar.gz`, the version without its `v`. */
export function assetName(tag) {
  return `sidevoice-web-${tag === "nightly" ? "nightly" : tag.slice(1)}.tar.gz`;
}

export function sha256Hex(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

/** Refuses a tarball whose digest is not the pin's, or that the release's SHA256SUMS does not list with that digest. */
export function checkDigest({ bytes, sums, asset, sha256 }) {
  const digest = sha256Hex(bytes);
  if (digest !== sha256) throw new Error(`${asset} is ${digest}, web.pin.json says ${sha256}`);
  const listed = sums
    .split("\n")
    .map((line) => line.trim().split(/\s+\*?/))
    .find(([, name]) => name === asset)?.[0];
  if (listed !== sha256) throw new Error(`SHA256SUMS lists ${asset} as ${listed ?? "nothing"}, not ${sha256}`);
}

/**
 * The pinned release's site, unpacked in `work/site`: downloaded through `fetch`, checked against the pin and
 * SHA256SUMS, and its attestation verified by `attest(tarball, bundle, repository)` when one is given. Kept: a second
 * call with the same pin only answers where it is.
 * @returns {Promise<string>} the site's directory
 */
export async function fetchSite(pin, { work, fetch = globalThis.fetch, attest = null }) {
  const { repository, tag, sha256 } = readPin(pin);
  const site = path.join(work, "site");
  const done = path.join(work, ".done");
  if (existsSync(done)) return site;
  rmSync(work, { recursive: true, force: true });
  mkdirSync(site, { recursive: true });
  const asset = assetName(tag);
  const base = `https://github.com/${repository}/releases/download/${tag}`;
  const download = async (name) => {
    const res = await fetch(`${base}/${name}`);
    if (!res.ok) throw new Error(`${repository} ${tag}: ${name} answered ${res.status}`);
    return new Uint8Array(await res.arrayBuffer());
  };
  const bytes = await download(asset);
  const sums = new TextDecoder().decode(await download("SHA256SUMS"));
  checkDigest({ bytes, sums, asset, sha256 });
  const tarball = path.join(work, asset);
  writeFileSync(tarball, bytes);
  if (attest) {
    const bundle = path.join(work, "attestation.sigstore.json");
    writeFileSync(bundle, await download("attestation.sigstore.json"));
    attest(tarball, bundle, repository);
  }
  execFileSync("tar", ["-xzf", tarball, "-C", site], { stdio: "inherit" });
  writeFileSync(done, "");
  return site;
}

/** `gh attestation verify` of `tarball` against its release's bundle and repository; throws when it fails. */
export function ghAttest(tarball, bundle, repository) {
  execFileSync("gh", ["attestation", "verify", tarball, "--bundle", bundle, "--repo", repository], { stdio: "inherit" });
}
