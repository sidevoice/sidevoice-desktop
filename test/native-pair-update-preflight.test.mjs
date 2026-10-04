import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { assertFreshAppConfig } from "./macos/native-pair-update-preflight.mjs";

test("candidate smoke preflight accepts missing or empty app config and refuses existing state", async () => {
  const fixture = await mkdtemp(join(tmpdir(), "sidevoice-pair-preflight-"));
  const absent = join(fixture, "absent");
  const empty = join(fixture, "empty");
  const populated = join(fixture, "populated");
  try {
    await assertFreshAppConfig(absent);
    await mkdir(empty);
    await assertFreshAppConfig(empty);
    await mkdir(populated);
    await writeFile(join(populated, "settings.json"), "{}\n");
    await assert.rejects(assertFreshAppConfig(populated), (error) => {
      assert.equal(error.message, "runner app configuration is not fresh.");
      assert.ok(error.message.length <= 120);
      assert.equal(error.message.includes(fixture), false, "diagnostics do not reveal fixture paths");
      return true;
    });
  } finally {
    await rm(fixture, { recursive: true, force: true });
  }
});
