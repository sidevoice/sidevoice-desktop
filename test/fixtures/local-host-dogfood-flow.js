// CI only: the vendored production page proves that its projected local host can reach the genuine pinned core.
(function () {
  "use strict";
  if (location.pathname !== "/voice/index.html" && location.pathname !== "/voice/") return;

  const say = (line) => window.__TAURI_INTERNALS__.invoke("debug_log", { line: "local-host-dogfood " + line });
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

  (async () => {
    try {
      const host = window.__sidevoiceDesktop?.host;
      const local = host?.localHost;
      if (!local) throw new Error("bridge-unavailable");

      const state = await until(async () => {
        const current = await local.state();
        return current.state === "running" && current.reachable === true && current.service === "launchd" ? current : null;
      }, 90000, "host-not-reachable");

      const pairing = await local.pairing();
      if (!pairing || pairing.local !== true || !/^http:\/\/127\.0\.0\.1:\d+$/.test(pairing.urls?.[0] || "")
        || typeof pairing.fp !== "string" || typeof pairing.device_id !== "string"
        || typeof pairing.token !== "string" || pairing.token.length !== 43) throw new Error("pairing-not-projected");

      // This is the same request the actual Machines view makes through the page-projected pairing.
      const listDevices = await until(async () => {
        const action = window.sidevoiceActions?.localHostDevices;
        if (typeof action !== "function") return null;
        try {
          const devices = await action();
          return Array.isArray(devices) && devices.some((device) =>
            device.device_id === pairing.device_id && device.current === true) ? devices : null;
        } catch { return null; }
      }, 60000, "page-proxy-did-not-reach-core");

      const before = await local.version();
      if (before.update !== "current" || before.installed?.connectorVersion !== before.bundled?.connector_version
        || before.installed?.coreVersion !== before.bundled?.core_version
        || state.core?.version !== before.bundled?.core_version) throw new Error("installed-build-not-current");

      const updated = await local.update();
      if (updated.result !== "noop" || updated.state !== "running" || updated.reachable !== true)
        throw new Error("same-version-update-not-safe-noop");
      const after = await local.version();
      const stillPaired = await local.pairing();
      if (after.update !== "current" || after.installed?.connectorVersion !== before.installed.connectorVersion
        || after.installed?.coreVersion !== before.installed.coreVersion || !stillPaired
        || stillPaired.device_id !== pairing.device_id || stillPaired.fp !== pairing.fp)
        throw new Error("same-version-update-changed-installed-host");

      // Never include the pairing token, device ID, code, host name, or response body in diagnostics.
      await say(`ok state=${state.state} reachable=${state.reachable} page-device=${listDevices.length > 0}
        update=${updated.result} connector=${after.installed.connectorVersion} core=${after.installed.coreVersion}`.replace(/\s+/g, " ").trim());
    } catch (error) {
      const key = typeof error?.message === "string" && /^[a-z0-9-]{1,64}$/.test(error.message)
        ? error.message : "unexpected-error";
      await say(`error ${key}`);
    }
  })();
})();
