#!/usr/bin/env node
// S0 spike: run on the paired machine to take the phone's run logs over the LAN. The launcher's "Send all runs" POSTs
// each file to /s0/<run>/<file>; they land in ./s0-results/<run>/<file> (or --dir). Stop it with Ctrl-C when done.
//
//   node receive-logs.mjs [--port 8799] [--dir ./s0-results]
import { createServer } from "node:http";
import { mkdirSync, writeFileSync } from "node:fs";
import { networkInterfaces } from "node:os";
import path from "node:path";

const arg = (name, fallback) => {
  const at = process.argv.indexOf(`--${name}`);
  return at > 0 ? process.argv[at + 1] : fallback;
};
const port = Number(arg("port", "8799"));
const dir = path.resolve(arg("dir", "./s0-results"));
const plain = /^[A-Za-z0-9._-]+$/;

createServer((req, res) => {
  res.setHeader("access-control-allow-origin", "*");
  const parts = decodeURIComponent(new URL(req.url, "http://x").pathname).split("/").filter(Boolean);
  if (req.method !== "POST" || parts.length !== 3 || parts[0] !== "s0" || !plain.test(parts[1]) || !plain.test(parts[2])) {
    res.writeHead(404).end();
    return;
  }
  const chunks = [];
  req.on("data", (c) => chunks.push(c));
  req.on("end", () => {
    const target = path.join(dir, parts[1]);
    mkdirSync(target, { recursive: true });
    writeFileSync(path.join(target, parts[2]), Buffer.concat(chunks));
    console.log(`received ${parts[1]}/${parts[2]} (${Buffer.concat(chunks).length} B)`);
    res.writeHead(200).end("ok");
  });
}).listen(port, "0.0.0.0", () => {
  const addresses = Object.values(networkInterfaces()).flat().filter((a) => a && a.family === "IPv4" && !a.internal);
  console.log(`S0 results go to ${dir}. On the phone, Results URL:`);
  for (const a of addresses) console.log(`  http://${a.address}:${port}`);
});
