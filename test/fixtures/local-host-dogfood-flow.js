// CI only: click the vendored local-install CTA and prove the page's bridge reaches its bundled local pair.
(function () {
  "use strict";
  if (location.pathname !== "/voice/index.html" && location.pathname !== "/voice/") return;

  const say = (line) => window.__TAURI_INTERNALS__.invoke("debug_log", { line: "local-host-dogfood " + line });
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const visible = (element) => Boolean(element && element.getClientRects().length
    && getComputedStyle(element).visibility !== "hidden");
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
    let local;
    let progressObserver;
    let sawInstallProgress = false;
    try {
      const host = window.__sidevoiceDesktop?.host;
      local = host?.localHost;
      if (!local || typeof local.install !== "function" || typeof local.version !== "function"
          || typeof local.update !== "function" || typeof local.cancel !== "function") throw new Error("bridge-unavailable");

      const initial = await local.state();
      if (!new Set(["absent", "not-installed"]).has(initial.state) || initial.installed === true || initial.reachable === true)
        throw new Error("runner-not-fresh");

      const remoteButton = await until(() => {
        const button = document.querySelector(".no-machine-actions > button");
        return visible(button) ? button : null;
      }, 120_000, "remote-pairing-button-missing");
      const localButton = await until(() => {
        const button = document.querySelector(".local-install-cta button");
        return visible(button) ? button : null;
      }, 120_000, "local-install-button-missing");

      // Preserve the remote-machine route: opening it still presents its code-entry dialog.
      remoteButton.click();
      await until(() => {
        const input = document.getElementById("device-pairing-code");
        return visible(input) ? input : null;
      }, 10_000, "remote-pairing-dialog-missing");
      const remoteCommand = document.querySelector(".remote-setup-guidance .setup-command code");
      if (!remoteCommand?.textContent?.includes("npx @sidevoice/uplink install")) throw new Error("remote-setup-guidance-missing");
      const close = document.getElementById("device-pairing-close");
      if (!visible(close)) throw new Error("remote-pairing-close-missing");
      close.click();
      await until(() => !visible(document.getElementById("device-pairing-code")), 10_000, "remote-pairing-dialog-stuck");

      const currentLocalButton = document.querySelector(".local-install-cta button");
      if (!visible(currentLocalButton)) throw new Error("local-install-button-disappeared");
      progressObserver = new MutationObserver(() => {
        const progress = document.querySelector(".local-install-panel .muted:not([role='status'])");
        if (visible(progress) && progress.textContent?.trim()) sawInstallProgress = true;
      });
      progressObserver.observe(document.body, { childList: true, characterData: true, subtree: true });
      currentLocalButton.click();
      const panel = await until(() => {
        const value = document.querySelector(".local-install-panel");
        return visible(value) && value.querySelector('[role="status"]') ? value : null;
      }, 15_000, "install-progress-panel-missing");
      const initialStatus = panel.querySelector('[role="status"]')?.textContent?.trim() || "";
      await until(() => sawInstallProgress, 60_000, "install-progress-not-rendered");

      const state = await until(async () => {
        if (panel.querySelector('[role="alert"]')) throw new Error("install-ui-reported-failure");
        const current = await local.state();
        return current.state === "running" && current.reachable === true && current.service === "launchd" ? current : null;
      }, 35 * 60_000, "local-host-not-reachable");
      await until(async () => (await local.version()).update === "current", 60_000, "install-bridge-did-not-complete");
      await until(() => {
        const status = panel.querySelector('[role="status"]')?.textContent?.trim();
        if (status && status !== initialStatus) return "status-updated";
        // The reviewed room selects the newly reachable local machine and removes this panel on success.
        return visible(panel) ? null : "advanced-to-local-room";
      }, 60_000, "install-ui-no-result");

      const pairing = await local.pairing();
      if (!pairing || pairing.local !== true || !/^http:\/\/127\.0\.0\.1:\d+$/.test(pairing.urls?.[0] || "")
          || typeof pairing.fp !== "string" || typeof pairing.device_id !== "string"
          || typeof pairing.token !== "string" || pairing.token.length !== 43) throw new Error("local-pairing-not-projected");

      const pageDevices = await until(async () => {
        const action = window.sidevoiceActions?.localHostDevices;
        if (typeof action !== "function") return null;
        try {
          const devices = await action();
          return Array.isArray(devices) && devices.some((device) =>
            device.device_id === pairing.device_id && device.current === true) ? devices : null;
        } catch { return null; }
      }, 60_000, "page-proxy-did-not-reach-core");

      const before = await local.version();
      const installedBuild = before.installed || {};
      const bundledBuild = before.bundled || {};
      const installedConnector = installedBuild.connectorVersion ?? installedBuild.connector_version;
      const installedCore = installedBuild.coreVersion ?? installedBuild.core_version;
      if (before.update !== "current" || installedConnector !== bundledBuild.connector_version
          || installedCore !== bundledBuild.core_version
          || installedBuild.runtimeKind !== bundledBuild.native_pair?.runtime_kind
          || installedBuild.pairId !== bundledBuild.native_pair?.pair_id
          || state.core?.version !== bundledBuild.core_version)
        throw new Error("installed-build-not-current");

      const updated = await local.update();
      if (updated.result !== "noop" || updated.state !== "running" || updated.reachable !== true)
        throw new Error("same-version-update-not-safe-noop");
      const after = await local.version();
      const stillPaired = await local.pairing();
      if (after.update !== "current" || !stillPaired || stillPaired.device_id !== pairing.device_id
          || stillPaired.fp !== pairing.fp)
        throw new Error("page-state-changed-after-update");

      await say("ok local-cta=true progress=rendered install-ui=settled remote-pairing=true reachable=true page-proxy=true update=noop");
    } catch (error) {
      const key = typeof error?.message === "string" && /^[a-z0-9-]{1,64}$/.test(error.message)
        ? error.message : "unexpected-error";
      await say(`error ${key}`);
    } finally {
      progressObserver?.disconnect();
    }
  })();
})();
