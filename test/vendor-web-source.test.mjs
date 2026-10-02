import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import assert from "node:assert/strict";

const webSource = JSON.parse(readFileSync(new URL("../ui/voice/web-source.json", import.meta.url), "utf8"));
const connectorPin = readFileSync(new URL("../src-tauri/connector-pin.json", import.meta.url));

test("the bundled production web is the exact reviewed R2/R3 head", () => {
  assert.equal(webSource.repository, "sidevoice/sidevoice-web");
  assert.equal(webSource.commit, "a0436e143ed5e29a81e86830f5902afde0df3786");
  assert.equal(webSource.uncommitted_changes, false);
});

test("R2/R3 web vendoring preserves the reviewed R4 connector pin", () => {
  const sha256 = createHash("sha256").update(connectorPin).digest("hex");
  assert.equal(sha256, "4cd98256aebb1bdc899a764dd11095f200087772af51e73808478e0f2723d7f2");
});
