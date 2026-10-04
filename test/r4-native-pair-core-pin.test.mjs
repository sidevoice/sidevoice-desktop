import assert from "node:assert/strict";
import { test } from "node:test";
import { alignCorePin } from "../scripts/align-r4-native-pair-core-pin.mjs";

const connectorSha = "435fd315e657a4f1372fc524f773387797195f38";
const oldCoreSha = "b41840e41e3eb81905d285514c7deb35bd8efe57";
const coreSha = "b2ae125453baa3634b94eefcc49879588e3b6e40";
const lockSha = "b0e068cf34f5c1549c00add4c2d94e06af3758bb1586c2e58f192220a22b53e9";
const original = { schema: 1, repository: "sidevoice/sidevoice-core", source_sha: oldCoreSha, cargo_lock_sha256: lockSha };
const target = {
  schema: 1,
  connector: { repository: "sidevoice/sidevoice-connector", source_sha: connectorSha },
  core: { repository: "sidevoice/sidevoice-core", source_sha: coreSha, cargo_lock_sha256: lockSha },
  target: "macos-aarch64",
};

test("candidate aligns only the exact stale Connector Core pin to the reviewed Desktop Core pin", () => {
  assert.deepEqual(alignCorePin(original, connectorSha, target), { ...original, source_sha: coreSha });
  assert.throws(() => alignCorePin({ ...original, source_sha: "0".repeat(40) }, connectorSha, target), /does not match/);
  assert.throws(() => alignCorePin(original, connectorSha, { ...target, core: { ...target.core, cargo_lock_sha256: "0".repeat(64) } }), /does not match/);
  assert.throws(() => alignCorePin(original, "0".repeat(40), target), /does not match/);
});
