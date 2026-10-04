import assert from "node:assert/strict";
import { test } from "node:test";
import { lstat, mkdir, mkdtemp, readFile, rm, symlink, unlink, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { findCommittedPairRelease } from "./macos/committed-pair-release.mjs";

test("rollback smoke locates the committed staged pair and rejects partial or linked lookalikes", async () => {
  const fixture = await mkdtemp(join(tmpdir(), "sidevoice-pair-stage-"));
  const releases = join(fixture, "releases");
  const oldPairId = "pair-old";
  const currentPairId = "pair-current";
  const committed = join(releases, "current-release");
  const partial = join(releases, "current-release.tmp-deadbeef");
  const outside = join(fixture, "outside-release");
  const linked = join(releases, "linked-release");
  try {
    await mkdir(committed, { recursive: true });
    await mkdir(join(releases, "old-release"));
    await mkdir(partial);
    await mkdir(outside);
    await writeFile(join(committed, "release.json"), JSON.stringify({ id: "current-release", pair_id: currentPairId }));
    await writeFile(join(releases, "old-release", "release.json"), JSON.stringify({ id: "old-release", pair_id: oldPairId }));
    await writeFile(join(partial, "release.json"), JSON.stringify({ id: "current-release.tmp-deadbeef", pair_id: currentPairId }));
    let symlinkCount = 0;
    if (process.platform !== "win32") {
      await writeFile(join(outside, "release.json"), JSON.stringify({ id: "outside-release", pair_id: currentPairId }));
      await symlink(outside, linked, "dir");
      symlinkCount = 1;
    }

    await assert.rejects(findCommittedPairRelease(releases, "pair-missing"), (error) => {
      assert.match(error.message, new RegExp(`matches=0; releases=2; temporary=1; symlinks=${symlinkCount}; other=0; invalid_records=0`));
      assert.ok(error.message.length <= 240);
      assert.equal(error.message.includes(fixture), false, "diagnostics do not reveal temporary paths");
      assert.equal(error.message.includes(currentPairId), false, "diagnostics do not reveal pair identifiers");
      return true;
    });

    await assert.rejects(findCommittedPairRelease(releases, currentPairId), (error) => {
      assert.match(error.message, new RegExp(`matches=1; releases=2; temporary=1; symlinks=${symlinkCount}; other=0; invalid_records=0`));
      assert.equal(error.message.includes(fixture), false);
      return true;
    });
    assert.ok((await lstat(partial)).isDirectory(), "inspection leaves incomplete staging data untouched");
    await rm(partial, { recursive: true, force: true });
    if (symlinkCount) {
      await assert.rejects(findCommittedPairRelease(releases, currentPairId), (error) => {
        assert.match(error.message, /matches=1; releases=2; temporary=0; symlinks=1; other=0; invalid_records=0/);
        assert.equal(error.message.includes(fixture), false);
        return true;
      });
      await unlink(linked);
    }

    const duplicate = join(releases, "duplicate-release");
    await mkdir(duplicate);
    await writeFile(join(duplicate, "release.json"), JSON.stringify({ id: "duplicate-release", pair_id: currentPairId }));
    await assert.rejects(findCommittedPairRelease(releases, currentPairId), (error) => {
      assert.match(error.message, /matches=2; releases=3; temporary=0; symlinks=0; other=0; invalid_records=0/);
      assert.equal(error.message.includes(fixture), false);
      assert.equal(error.message.includes(currentPairId), false);
      return true;
    });
    await rm(duplicate, { recursive: true, force: true });
    assert.equal(await findCommittedPairRelease(releases, currentPairId), committed);
    assert.deepEqual(JSON.parse(await readFile(join(committed, "release.json"), "utf8")), { id: "current-release", pair_id: currentPairId });
    assert.ok((await lstat(join(releases, "old-release"))).isDirectory(), "inspection preserves the rollback release");
  } finally {
    await rm(fixture, { recursive: true, force: true });
  }
});
