import { fetchConnector } from "./connector-pin.mjs";

try {
  const result = await fetchConnector();
  process.stdout.write(`${JSON.stringify(result)}\n`);
} catch (error) {
  process.stderr.write(`Connector resource preparation failed: ${error.message}\n`);
  process.exitCode = 1;
}
