import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdir, mkdtemp, realpath, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";
import { assertReleaseRootWithin } from "./macos/release-root-containment.mjs";

test("release root containment accepts a temp-root symlink alias and still rejects outside roots", async () => {
  const fixture = await mkdtemp(join(tmpdir(), "sidevoice-release-root-"));
  try {
    const expectedRoot = join(fixture, "runner-temp");
    const tempAlias = join(fixture, "temp-alias");
    const releaseRoot = join(expectedRoot, "xdg-data", "sidevoice");
    const outsideRoot = join(fixture, "outside", "sidevoice");
    await mkdir(releaseRoot, { recursive: true });
    await mkdir(outsideRoot, { recursive: true });
    await symlink(expectedRoot, tempAlias, "dir");

    const canonicalReleaseRoot = await realpath(releaseRoot);
    assert.ok(relative(tempAlias, canonicalReleaseRoot).startsWith(".."),
      "the lexical alias comparison reproduces macOS temp path canonicalization");
    assert.equal(await assertReleaseRootWithin(releaseRoot, tempAlias), canonicalReleaseRoot);

    await assert.rejects(assertReleaseRootWithin(outsideRoot, tempAlias), (error) => {
      assert.equal(error.message, "install record release root is outside the isolated test data directory.");
      assert.ok(error.message.length <= 120);
      assert.equal(error.message.includes(fixture), false, "diagnostics do not reveal temporary paths");
      return true;
    });
  } finally {
    await rm(fixture, { recursive: true, force: true });
  }
});
