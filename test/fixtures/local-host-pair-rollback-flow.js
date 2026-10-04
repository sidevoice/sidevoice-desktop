// CI only: force a staged pair failure and verify Desktop reports the Connector rollback with the old pair reachable.
(function () {
  "use strict";
  if (location.pathname !== "/voice/index.html" && location.pathname !== "/voice/") return;

  const say = (line) => window.__TAURI_INTERNALS__.invoke("debug_log", { line: "local-host-pair-rollback " + line });
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
  const errorKey = (error) => {
    const direct = error?.key ?? error?.error?.key;
    if (typeof direct === "string") return direct;
    const message = typeof error === "string" ? error : error?.message;
    if (typeof message === "string") {
      try {
        const parsed = JSON.parse(message);
        if (typeof parsed?.key === "string") return parsed.key;
      } catch {}
      const match = /\binstall\.rollback\b/.exec(message);
      if (match) return match[0];
    }
    return "unknown";
  };

  (async () => {
    try {
      const local = window.__sidevoiceDesktop?.host?.localHost;
      if (!local || typeof local.version !== "function" || typeof local.update !== "function")
        throw new Error("bridge-unavailable");
      await until(async () => {
        const current = await local.state();
        return current.state === "running" && current.reachable === true && current.service === "launchd" ? current : null;
      }, 60_000, "prior-pair-not-reachable");
      const pairing = await until(() => local.pairing(), 20_000, "prior-pairing-missing");
      const before = await local.version();
      const previousPair = field(before.installed, "pairId", "pair_id");
      const bundledPair = before.bundled?.native_pair?.pair_id;
      if (before.update !== "available" || !previousPair || !bundledPair || previousPair === bundledPair)
        throw new Error("prior-pair-not-eligible");

      let failureKey = null;
      try { await local.update(); }
      catch (error) { failureKey = errorKey(error); }
      if (failureKey !== "install.rollback") throw new Error("connector-did-not-report-rollback");
      const recovered = await until(async () => {
        const [currentState, version] = await Promise.all([local.state(), local.version()]);
        return currentState.state === "running" && currentState.reachable === true
          && field(version.installed, "pairId", "pair_id") === previousPair ? { currentState, version } : null;
      }, 60_000, "previous-pair-not-reachable-after-rollback");
      const stillPaired = await local.pairing();
      const recoveredCore = recovered.currentState.core?.version;
      const selectedCore = field(recovered.version.installed, "coreVersion", "core_version");
      if (!stillPaired || stillPaired.device_id !== pairing.device_id || stillPaired.fp !== pairing.fp
          || recovered.version.update !== "available" || recovered.currentState.service !== "launchd"
          || !selectedCore || recoveredCore !== selectedCore || selectedCore !== before.bundled?.core_version)
        throw new Error("rollback-state-mismatch");

      await say("ok error=install.rollback previous-pair=true reachable=true pairing-preserved=true");
    } catch (error) {
      const key = typeof error?.message === "string" && /^[a-z0-9-]{1,64}$/.test(error.message)
        ? error.message : "unexpected-error";
      await say(`error ${key}`);
    }
  })();
})();
