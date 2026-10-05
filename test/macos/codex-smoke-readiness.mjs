import { relative, resolve } from "node:path";

export const CODEX_CLI_VERSION = "0.160.0";

export function assertCodexCliVersion(output) {
  const version = String(output || "").trim();
  if (!new RegExp(`(?:^|\\s)${CODEX_CLI_VERSION.replaceAll(".", "\\.")}(?:\\s|$)`).test(version)) {
    throw new Error(`Codex CLI is not the pinned ${CODEX_CLI_VERSION} build.`);
  }
  return CODEX_CLI_VERSION;
}

export function assertCodexCliPath(binary, runnerTemp) {
  if (typeof binary !== "string" || typeof runnerTemp !== "string") {
    throw new Error("Codex CLI or runner-temp location is unavailable.");
  }
  const expected = resolve(runnerTemp, "codex-cli/node_modules/.bin/codex");
  if (resolve(binary || "") !== expected) {
    throw new Error("Codex CLI is not the pinned runner-temp executable.");
  }
  return expected;
}

export function assertDisposableCodexHome(codexHome, runnerTemp, userHome) {
  const home = resolve(codexHome || "");
  const relativeHome = relative(resolve(runnerTemp || ""), home);
  if (relativeHome !== "r4-codex-home" || home === resolve(userHome || "", ".codex")) {
    throw new Error("Codex CLI profile is not isolated inside this runner's temporary directory.");
  }
  return home;
}

export function assertConnectedCodexReport(report) {
  if (report?.ok === false || report?.error || !Array.isArray(report?.agents)) {
    throw new Error("Installed Connector returned no Codex agent report.");
  }
  const codex = report.agents.filter(agent => agent?.id === "codex");
  if (codex.length !== 1 || codex[0].registration !== "connected") {
    throw new Error("Installed Connector did not confirm exactly one connected Codex registration.");
  }
  return codex[0];
}
