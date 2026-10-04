// CI only: drive a changed Rust pair update through the packaged room page and Desktop bridge.
(function () {
  "use strict";
  if (location.pathname !== "/voice/index.html" && location.pathname !== "/voice/") return;

  const say = (line) => window.__TAURI_INTERNALS__.invoke("debug_log", { line: "local-host-pair-update " + line });
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const until = async (check, timeout, key) => {
    const end = Date.now() + timeout;
    while (Date.now() < end) {
      const value = await check();
      if (value) return value;
      await sleep(250);
    }
    throw new Error(key);
  };
  const field = (object, camel, snake) => object?.[camel] ?? object?.[snake];

  (async () => {
    try {
      const local = window.__sidevoiceDesktop?.host?.localHost;
      if (!local || typeof local.version !== "function" || typeof local.update !== "function")
        throw new Error("bridge-unavailable");
      const state = await until(async () => {
        const current = await local.state();
        return current.state === "running" && current.reachable === true && current.service === "launchd" ? current : null;
      }, 60_000, "prior-pair-not-reachable");
      const pairing = await until(() => local.pairing(), 20_000, "prior-pairing-missing");
      const before = await local.version();
      const installedPair = field(before.installed, "pairId", "pair_id");
      const bundledPair = before.bundled?.native_pair?.pair_id;
      const installedRuntime = field(before.installed, "runtimeKind", "runtime_kind");
      const installedCore = field(before.installed, "coreVersion", "core_version");
      if (before.update !== "available" || installedRuntime !== "rust-native-v1"
          || installedCore !== before.bundled?.core_version || !installedPair || installedPair === bundledPair)
        throw new Error("prior-pair-not-eligible");

      const result = await local.update();
      if (result.result !== "upgrade" || result.state !== "running" || result.reachable !== true)
        throw new Error("changed-pair-update-not-reachable");
      const after = await until(async () => {
        const current = await local.version();
        return current.update === "current" ? current : null;
      }, 60_000, "changed-pair-update-not-current");
      const currentPair = field(after.installed, "pairId", "pair_id");
      const stillPaired = await local.pairing();
      if (!bundledPair || currentPair !== bundledPair || !stillPaired
          || stillPaired.device_id !== pairing.device_id || stillPaired.fp !== pairing.fp
          || state.core?.version !== after.bundled?.core_version)
        throw new Error("changed-pair-update-identity-mismatch");

      await say("ok action=upgrade changed-pair=true reachable=true pairing-preserved=true");
    } catch (error) {
      const key = typeof error?.message === "string" && /^[a-z0-9-]{1,64}$/.test(error.message)
        ? error.message : "unexpected-error";
      await say(`error ${key}`);
    }
  })();
})();
