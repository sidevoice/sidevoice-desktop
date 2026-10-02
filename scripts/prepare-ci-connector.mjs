import { constants } from "node:fs";
import { appendFile, chmod, copyFile, mkdir, readFile, stat, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fetchConnector } from "./connector-pin.mjs";
import { fixtureResponses, sha256 } from "./connector-package-fixture.mjs";

const expectedPath = process.argv[2];
if (!expectedPath) throw new Error("Pass a temporary path for the CI connector fixture identity.");
const pinPath = resolve("src-tauri/connector-pin.json");
const pin = JSON.parse(await readFile(pinPath, "utf8"));
let fixture = false;

if (pin.status === "ready") {
  await fetchConnector();
} else if (pin.status === "pending") {
  const source = resolve("test/fixtures/connector-package-cli.sh");
  const destination = resolve("src-tauri/resources/sidevoice");
  await mkdir(resolve("src-tauri/resources"), { recursive: true });
  await copyFile(source, destination, constants.COPYFILE_EXCL); // Never replace a real or previous connector resource.
  await chmod(destination, 0o755);
  const bytes = await readFile(destination);
  const info = await stat(destination);
  await writeFile(expectedPath, `${JSON.stringify({
    fixture: "sidevoice-r4-c-package-fixture-v1",
    size: info.size,
    sha256: sha256(bytes),
    responses: fixtureResponses,
  }, null, 2)}\n`, { flag: "wx" });
  fixture = true;
} else {
  throw new Error("The connector pin is neither ready nor explicitly pending; refusing CI packaging.");
}

if (process.env.GITHUB_OUTPUT) {
  await appendFile(process.env.GITHUB_OUTPUT, `fixture=${fixture}\n`);
}
process.stdout.write(`${JSON.stringify({ fixture })}\n`);
