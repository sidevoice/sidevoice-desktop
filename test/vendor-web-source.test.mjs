import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import assert from "node:assert/strict";

const webSource = JSON.parse(readFileSync(new URL("../ui/voice/web-source.json", import.meta.url), "utf8"));
const connectorPin = readFileSync(new URL("../src-tauri/connector-pin.json", import.meta.url));

test("the bundled production web is the exact reviewed R2/R3 head", () => {
  assert.equal(webSource.repository, "sidevoice/sidevoice-web");
  assert.equal(webSource.commit, "c2cc354f9b00dac1b0cad5fb70bb043751190103");
  assert.equal(webSource.uncommitted_changes, false);
});

test("R2/R3 web vendoring preserves the reviewed R2 local-host connector pin", () => {
  const sha256 = createHash("sha256").update(connectorPin).digest("hex");
  assert.equal(sha256, "32b59a536e6548960b725cfd97e35b20a8bd86c9b3093553dbf50a4674558734");
});
