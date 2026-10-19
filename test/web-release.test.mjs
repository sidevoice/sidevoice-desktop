// scripts/web-release.mjs: the pin names a release and its tarball's digest; the tarball is downloaded, checked
// against the pin and the release's SHA256SUMS, its attestation verified when asked, unpacked once, and kept.
import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

import { assetName, checkDigest, fetchSite, readPin, sha256Hex } from "../scripts/web-release.mjs";

const SHA = "a".repeat(64);
const pin = (over = {}) => ({ repository: "sidevoice/sidevoice-web", tag: "v0.2.0", sha256: SHA, ...over });

test("a pin names a repository, a release tag and the tarball's full SHA-256", () => {
  assert.deepEqual(readPin(pin()), pin());
  assert.deepEqual(readPin(pin({ tag: "nightly" })).tag, "nightly");
  assert.deepEqual(readPin(pin({ tag: "v0.2.0-rc.1" })).tag, "v0.2.0-rc.1");
  for (const bad of [
    pin({ repository: "no-owner" }),
    pin({ tag: "main" }),
    pin({ tag: "0.2.0" }),
    pin({ sha256: "abc" }),
    { repository: "sidevoice/sidevoice-web", commit: "22686ca1f6801525d50f55724664fa86d54802b5" },
  ]) {
    assert.throws(() => readPin(bad), /web\.pin\.json/, JSON.stringify(bad));
  }
});

test("a release's tarball is named by its version, the nightly's by nightly", () => {
  assert.equal(assetName("v0.2.0"), "sidevoice-web-0.2.0.tar.gz");
  assert.equal(assetName("nightly"), "sidevoice-web-nightly.tar.gz");
});

test("a tarball is taken only with the pin's digest, listed so in SHA256SUMS", () => {
  const bytes = new TextEncoder().encode("a site");
  const sha256 = sha256Hex(bytes);
  const asset = "sidevoice-web-0.2.0.tar.gz";
  checkDigest({ bytes, sums: `${sha256}  ${asset}\n`, asset, sha256 });
  assert.throws(() => checkDigest({ bytes, sums: `${sha256}  ${asset}\n`, asset, sha256: SHA }), /web\.pin\.json says/);
  assert.throws(() => checkDigest({ bytes, sums: `${SHA}  ${asset}\n`, asset, sha256 }), /SHA256SUMS lists/);
  assert.throws(() => checkDigest({ bytes, sums: `${sha256}  other.tar.gz\n`, asset, sha256 }), /lists .* as nothing/);
});

/** A release's assets, as a tarball of `files` with its SHA256SUMS and an attestation bundle. */
function release(files) {
  const work = mkdtempSync(path.join(tmpdir(), "web-release-"));
  const tree = path.join(work, "tree");
  for (const [file, text] of Object.entries(files)) {
    mkdirSync(path.dirname(path.join(tree, file)), { recursive: true });
    writeFileSync(path.join(tree, file), text);
  }
  const tarball = path.join(work, "site.tar.gz");
  execFileSync("tar", ["-czf", tarball, "-C", tree, "."]);
  const bytes = readFileSync(tarball);
  return { work, bytes, sha256: sha256Hex(bytes) };
}

test("the pinned release is downloaded, checked, attested, unpacked once, and kept", async () => {
  const { work, bytes, sha256 } = release({ "index.html": "root", "voice/index.html": "room" });
  const asset = assetName("v0.2.0");
  const base = "https://github.com/sidevoice/sidevoice-web/releases/download/v0.2.0";
  const served = {
    [`${base}/${asset}`]: bytes,
    [`${base}/SHA256SUMS`]: new TextEncoder().encode(`${sha256}  ${asset}\n`),
    [`${base}/attestation.sigstore.json`]: new TextEncoder().encode("{}"),
  };
  const asked = [];
  const fetch = async (url) => {
    asked.push(url);
    return served[url] ? new Response(served[url]) : new Response("", { status: 404 });
  };
  const attested = [];
  const into = path.join(work, "cache");
  const site = await fetchSite(pin({ sha256 }), {
    work: into,
    fetch,
    attest: (tarball, bundle, repository) => attested.push([path.basename(tarball), path.basename(bundle), repository]),
  });
  assert.equal(readFileSync(path.join(site, "voice/index.html"), "utf8"), "room");
  assert.deepEqual(attested, [[asset, "attestation.sigstore.json", "sidevoice/sidevoice-web"]]);
  assert.equal(asked.length, 3);

  // The same pin again: nothing is downloaded.
  assert.equal(await fetchSite(pin({ sha256 }), { work: into, fetch }), site);
  assert.equal(asked.length, 3);

  // A failed attestation stops it, and nothing is kept as done.
  const failed = path.join(work, "failed");
  await assert.rejects(
    fetchSite(pin({ sha256 }), {
      work: failed,
      fetch,
      attest: () => {
        throw new Error("attestation refused");
      },
    }),
    /attestation refused/,
  );
  assert.equal(existsSync(path.join(failed, ".done")), false);

  // A tarball that is not the pinned one is refused before it is unpacked.
  const wrong = path.join(work, "wrong");
  await assert.rejects(fetchSite(pin(), { work: wrong, fetch }), /web\.pin\.json says/);
  assert.equal(existsSync(path.join(wrong, "site/voice")), false);
});
