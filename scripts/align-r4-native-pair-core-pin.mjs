import { execFileSync } from "node:child_process";
import { lstat, readFile, writeFile, mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const CONNECTOR_SHAS = new Set([
  "435fd315e657a4f1372fc524f773387797195f38",
  "3cf94b46fce86ade5f182dd8da17d37277188221",
  "cdcc1a613d35bf600b8f39427195cbe4700ea6ec",
  "6076bdab84ed6db23ba8735ff669050a5728ed32",
  "47c77105372c72ee5cb90eb40bb22ad392262a7d",
  "c3aa3468e66e265897fdf0fccf5ad55d6fcf060f",
]);
const OLD_CORE_SHA = "b41840e41e3eb81905d285514c7deb35bd8efe57";
const CORE_SHA = "b2ae125453baa3634b94eefcc49879588e3b6e40";
const CORE_LOCK_SHA256 = "b0e068cf34f5c1549c00add4c2d94e06af3758bb1586c2e58f192220a22b53e9";
const CORE_PIN_PATH = "packages/connector/rust-core-production-pin.json";

function exactKeys(value, keys) {
  return value && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).sort().join(",") === [...keys].sort().join(",");
}

export function alignCorePin(committedPin, connectorSha, targetPin) {
  if (!CONNECTOR_SHAS.has(connectorSha)
      || !exactKeys(committedPin, ["schema", "repository", "source_sha", "cargo_lock_sha256"])
      || committedPin.schema !== 1 || committedPin.repository !== "sidevoice/sidevoice-core"
      || committedPin.source_sha !== OLD_CORE_SHA || committedPin.cargo_lock_sha256 !== CORE_LOCK_SHA256
      || targetPin?.schema !== 1 || targetPin.connector?.repository !== "sidevoice/sidevoice-connector"
      || targetPin.core?.repository !== "sidevoice/sidevoice-core" || targetPin.target !== "macos-aarch64"
      || targetPin?.core?.source_sha !== CORE_SHA || targetPin.core.cargo_lock_sha256 !== CORE_LOCK_SHA256
      || targetPin.connector?.source_sha !== connectorSha) {
    throw new Error("Connector Core source pin does not match the reviewed R4 trial pair.");
  }
  return { ...committedPin, source_sha: CORE_SHA, cargo_lock_sha256: CORE_LOCK_SHA256 };
}

async function alignConnectorCheckout(connectorRoot, connectorSha, sourcePinPath, evidencePath) {
  const root = resolve(connectorRoot);
  const head = execFileSync("git", ["-C", root, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();
  if (head !== connectorSha || !CONNECTOR_SHAS.has(connectorSha)) {
    throw new Error("Connector checkout is not the exact reviewed R4 source.");
  }
  const dirty = execFileSync("git", ["-C", root, "status", "--porcelain", "--", CORE_PIN_PATH], { encoding: "utf8" }).trim();
  if (dirty) throw new Error("Connector Core source pin has unexpected local changes.");
  const committedPin = JSON.parse(execFileSync("git", ["-C", root, "show", `${connectorSha}:${CORE_PIN_PATH}`], { encoding: "utf8" }));
  const sourcePin = JSON.parse(await readFile(resolve(sourcePinPath), "utf8"));
  const file = resolve(root, CORE_PIN_PATH);
  const info = await lstat(file);
  if (!info.isFile() || info.isSymbolicLink()) throw new Error("Connector Core source pin is not a regular file.");
  const currentPin = JSON.parse(await readFile(file, "utf8"));
  if (JSON.stringify(currentPin) !== JSON.stringify(committedPin)) {
    throw new Error("Connector Core source pin differs from the pinned commit.");
  }
  const alignedPin = alignCorePin(committedPin, connectorSha, sourcePin);
  await writeFile(file, `${JSON.stringify(alignedPin, null, 2)}\n`);
  const evidence = {
    schema: 1,
    kind: "r4-native-pair-candidate-core-pin-alignment",
    connector_source_sha: connectorSha,
    committed_core_source_sha: committedPin.source_sha,
    effective_core_source_sha: alignedPin.source_sha,
    core_cargo_lock_sha256: alignedPin.cargo_lock_sha256,
  };
  await mkdir(dirname(resolve(evidencePath)), { recursive: true });
  await writeFile(resolve(evidencePath), `${JSON.stringify(evidence, null, 2)}\n`, { mode: 0o600 });
  return evidence;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [connectorRoot, connectorSha, sourcePinPath, evidencePath] = process.argv.slice(2);
  if (process.argv.length !== 6) {
    throw new Error("Usage: align-r4-native-pair-core-pin <connector-root> <connector-sha> <desktop-source-pin> <evidence-file>");
  }
  const evidence = await alignConnectorCheckout(connectorRoot, connectorSha, sourcePinPath, evidencePath);
  process.stdout.write(`${JSON.stringify(evidence)}\n`);
}
