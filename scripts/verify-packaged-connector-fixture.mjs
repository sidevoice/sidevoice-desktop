import { resolve } from "node:path";
import { verifyConnectorFixture } from "./connector-package-fixture.mjs";

const [executable, expectedPath] = process.argv.slice(2);
if (!executable || !expectedPath) throw new Error("Pass the packaged resources/sidevoice path and fixture identity path.");
const result = await verifyConnectorFixture(executable, expectedPath, resolve("src-tauri/connector-pin.json"));
process.stdout.write(`${JSON.stringify(result)}\n`);
