// CI only, injected by the probe build into the real bundled /voice/index.html page. It seeds one fake remote
// pairing, opens the marked Settings gear, follows Settings → Machines → that host's Agents tab, and clicks Connect.
// The fake host requires this pairing's token for /api/host/agents; diagnostics report only a boolean auth result.
(function () {
  "use strict";
  if (location.pathname !== "/voice/index.html" && location.pathname !== "/voice/") return;

  const MACHINE = "XO8Z6hj1_KrQXjxM2FXpBPd6qOyC__Dfc4zLn-4dOwI";
  const MACHINE_KEY = "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEe+HC+jHE39mN/AXjcJyjxUr/FeJ335haZ4Kgjoj6ZhZm90SXgy1EH2nNFOxZTXr4P892sl5p+eiDv8sLhAOywg==";
  const TOKEN = "ci-agents-device-token";
  const BASE = "http://127.0.0.1:8768";
  const seen = [];
  const presentationReads = [];
  const presentationJsonStates = new WeakMap();
  const dialogAttempts = [];
  const nativeFetch = window.fetch.bind(window);
  if (typeof Response !== "undefined" && typeof Response.prototype.json === "function") {
    const nativeResponseJson = Response.prototype.json;
    Response.prototype.json = function (...args) {
      const presentationRead = presentationJsonStates.get(this);
      if (!presentationRead) return nativeResponseJson.apply(this, args);
      let parsed;
      try {
        parsed = nativeResponseJson.apply(this, args);
      } catch (error) {
        presentationRead.json = "rejected";
        throw error;
      }
      return parsed.then((body) => {
        presentationRead.json = "resolved";
        return body;
      }, (error) => {
        presentationRead.json = "rejected";
        throw error;
      });
    };
  }
  const showModalAvailable = typeof HTMLDialogElement !== "undefined"
    && typeof HTMLDialogElement.prototype.showModal === "function";
  if (showModalAvailable) {
    const nativeShowModal = HTMLDialogElement.prototype.showModal;
    HTMLDialogElement.prototype.showModal = function (...args) {
      const attempt = { id: this.id, outcome: "called" };
      dialogAttempts.push(attempt);
      try {
        const result = nativeShowModal.apply(this, args);
        attempt.outcome = this.open ? "opened" : "returned-closed";
        return result;
      } catch (error) {
        attempt.outcome = "threw";
        throw error;
      }
    };
  }

  // Record only endpoint, method, and whether the exact host pairing token was used. Never retain or print a token.
  window.fetch = function (input, init = {}) {
    const request = input instanceof Request ? input : null;
    const url = new URL(request ? request.url : String(input), location.href);
    let presentationRead = null;
    if (url.origin === BASE && url.pathname.startsWith("/api/host/agents")) {
      const headers = new Headers(request?.headers);
      new Headers(init.headers).forEach((value, key) => headers.set(key, value));
      seen.push({ path: url.pathname + url.search, method: (init.method || request?.method || "GET").toUpperCase(),
        authorized: headers.get("authorization") === `Bearer ${TOKEN}` });
    } else if (url.origin === BASE && url.pathname === "/api/presentation/languages") {
      presentationRead = { status: "pending", json: "not-requested" };
      presentationReads.push(presentationRead);
    }
    const response = nativeFetch(input, init);
    if (!presentationRead) return response;
    return response.then((value) => {
      presentationRead.status = `http-${value.status}`;
      presentationRead.json = "pending";
      presentationJsonStates.set(value, presentationRead);
      return value;
    }, (error) => {
      presentationRead.status = "rejected";
      presentationRead.json = "not-available";
      throw error;
    });
  };

  // Set before the web bundle reads its persisted pairings, as the native-worker fixture does.
  localStorage.removeItem("sidevoice.stages");
  localStorage.removeItem("sidevoice.settings");
  localStorage.setItem("sidevoice.pairings", JSON.stringify({ in_use: MACHINE, pairings: [{
    fp: MACHINE, token: TOKEN, public_key: MACHINE_KEY, device_id: "ci-agents-device",
    urls: [BASE], rv: null, host: "CI Agents Host", paired_at: Date.now(),
  }] }));

  const say = (line) => window.__TAURI_INTERNALS__.invoke("debug_log", { line: "r2-r3-agents-flow " + line });
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  async function until(check, timeout, name) {
    const end = Date.now() + timeout;
    while (Date.now() < end) {
      const value = check();
      if (value) return value;
      await sleep(100);
    }
    throw new Error("timeout-" + name);
  }
  const store = () => window.sidevoiceUI?.store?.getState();
  const listing = () => store()?.facts?.hostAgents?.[MACHINE];
  const findButton = (selector, label) => [...document.querySelectorAll(selector)].find((button) =>
    button.textContent.trim() === label || button.getAttribute("aria-label") === label);
  const safeName = (error) => /^[a-z0-9-]{1,64}$/.test(error?.message || "") ? error.message : "unexpected-error";

  (async () => {
    try {
      await until(() => window.sidevoiceActions && window.sidevoiceUI && listing()?.status === "ready"
        && listing()?.value?.agents?.some((agent) => agent.id === "cursor" && agent.actionable), 60000, "host-agent-scan");

      const gear = document.getElementById("settings-open");
      if (!gear || !gear.getAttribute("aria-label")?.toLowerCase().includes("agents")) throw new Error("unmarked-settings-gear");
      const nativeClickHandler = gear.onclick;
      const handlerState = { invoked: false, outcome: "not-called" };
      if (typeof nativeClickHandler === "function") {
        gear.onclick = function (...args) {
          handlerState.invoked = true;
          handlerState.outcome = "running";
          const result = nativeClickHandler.apply(this, args);
          if (result && typeof result.then === "function") {
            result.then(() => { handlerState.outcome = "resolved"; }, () => { handlerState.outcome = "rejected"; });
          } else {
            handlerState.outcome = "resolved";
          }
          return result;
        };
      }
      gear.click();
      const settingsDialog = document.getElementById("language-settings");
      const settingsError = document.getElementById("settings-error");
      if (!settingsDialog || !settingsError) throw new Error("settings-dialog-controls-missing");
      await sleep(750);
      if (!settingsDialog.open) {
        const requiredSettings = ["ui-language", "audio-grace-seconds", "replay-on-return-seconds", "turn-patience", "presence-sound", "locked-call"];
        const missing = requiredSettings.filter((id) => !document.getElementById(id));
        const reads = presentationReads.map((entry) => `${entry.status}/${entry.json}`).join(",") || "none";
        const modal = dialogAttempts.map((entry) => `${entry.id || "unknown"}:${entry.outcome}`).join(",") || "not-called";
        await say(`settings-state dialog=closed connected=${settingsDialog.isConnected ? "yes" : "no"} click-handler=${typeof gear.onclick === "function" ? "yes" : "no"} handler-invoked=${handlerState.invoked ? "yes" : "no"} handler-outcome=${handlerState.outcome} dialog-api=${showModalAvailable ? "available" : "unavailable"} boot-error=${store()?.facts?.bootError ? "present" : "none"} missing-fields=${missing.join(",") || "none"} error=${settingsError.textContent.trim() ? "yes" : "no"} agent-request=${store()?.facts?.settingsAgentRequest?.id ? "yes" : "no"} language-fetch=${reads} show-modal=${modal}`);
      }
      try {
        await until(() => settingsDialog.open || !!settingsError.textContent.trim(), 15000, "settings-dialog");
      } catch (error) {
        if (error?.message === "timeout-settings-dialog" && !settingsDialog.open) {
          const reads = presentationReads.map((entry) => `${entry.status}/${entry.json}`).join(",") || "none";
          await say(`settings-timeout handler-invoked=${handlerState.invoked ? "yes" : "no"} handler-outcome=${handlerState.outcome} dialog-api=${showModalAvailable ? "available" : "unavailable"} boot-error=${store()?.facts?.bootError ? "present" : "none"} language-fetch=${reads} show-modal=${dialogAttempts.map((entry) => `${entry.id || "unknown"}:${entry.outcome}`).join(",") || "not-called"}`);
        }
        throw error;
      }
      if (!settingsDialog.open) {
        throw new Error("settings-dialog-error");
      }
      const machinesTab = document.getElementById("settings-machines");
      if (!machinesTab) throw new Error("settings-machines-control-missing");
      machinesTab.click();
      await until(() => machinesTab.getAttribute("aria-pressed") === "true"
        && document.getElementById("pane-machines")?.hidden === false, 15000, "settings-machines");
      if (!document.querySelector(".host-detail")) {
        const hostAgentButton = [...document.querySelectorAll(".machine-row .host-agent-tab-label")].find((button) =>
          button.textContent.trim() === "Agents" || button.getAttribute("aria-label")?.includes("Agents"));
        if (!hostAgentButton) throw new Error("host-agents-action-missing");
        hostAgentButton.click();
      }
      await until(() => document.querySelector(".host-detail")
        && document.getElementById("host-detail-title")?.textContent.includes("CI Agents Host"), 15000, "host-detail");
      await until(() => document.getElementById("host-tab-agents")?.getAttribute("aria-selected") === "true"
        && document.querySelector('.host-agent-row[data-registration="not-connected"]'), 15000, "host-agents-tab");

      const connect = findButton(".host-agent-row[data-registration='not-connected'] button", "Connect");
      if (!connect) throw new Error("connect-action-missing");
      connect.click();
      await until(() => listing()?.value?.agents?.some((agent) => agent.id === "cursor" && agent.registration === "connected"),
        15000, "connect-result");
      const connected = document.querySelector('.host-agent-row[data-registration="connected"]');
      if (!connected) throw new Error("connected-row-not-rendered");
      await until(() => !gear.getAttribute("aria-label")?.toLowerCase().includes("agents"), 5000, "gear-notice-cleared");

      const foreign = document.querySelector('.host-agent-row[data-registration="foreign"]');
      const disclosure = foreign && [...foreign.querySelectorAll("button")].find((button) =>
        button.textContent.trim() === "Replace manually" && foreign.textContent.includes("Codex"));
      if (!disclosure) throw new Error("foreign-manual-disclosure-missing");
      disclosure.click();
      const instructions = await until(() => document.querySelector('.host-agent-row[data-registration="foreign"] .host-agent-howto'),
        5000, "foreign-instructions");
      if (!instructions.textContent.includes("'/opt/homebrew/bin/codex' 'mcp' 'remove' 'sidevoice'")
        || !instructions.textContent.includes("'/opt/homebrew/bin/codex' 'mcp' 'add' 'sidevoice'")
        || !instructions.querySelector('button[aria-label="Copy"]')) throw new Error("foreign-manual-instructions-missing");
      let copyResult = "unavailable";
      const copy = instructions.querySelector('button[aria-label="Copy"]');
      if (typeof navigator.clipboard?.writeText === "function") {
        copy.click();
        const copyNote = await until(() => {
          const note = copy.querySelector('[role="status"]')?.textContent.trim();
          return note === "Copied" || typeof note === "string" && note.startsWith("Could not copy") ? note : null;
        }, 2500, "copy-feedback").catch(() => null);
        copyResult = copyNote === "Copied" ? "available" : copyNote ? "not-permitted" : "unconfirmed";
      }

      const get = seen.find((item) => item.method === "GET" && item.path.startsWith("/api/host/agents?"));
      const post = seen.find((item) => item.method === "POST" && item.path === "/api/host/agents/cursor/connect");
      if (!get || !post || !get.authorized || !post.authorized) throw new Error("host-route-auth-mismatch");
      if (!document.querySelector('.machine-row[data-agent-notice="true"]')) {
        // The focused HostPage hides its list; the store remains the source of its host-specific notice state.
        if (listing()?.value?.agents?.some((agent) => agent.actionable)) throw new Error("agent-notice-did-not-clear");
      }

      // Expire the fake pairing through a test-only endpoint, then let the production host API receive its real 401.
      // This checks that the live Agents tab falls back to Status when the selected pairing is revoked.
      await nativeFetch(BASE + "/__ci/revoke-host");
      await window.sidevoiceActions.loadHostAgents(MACHINE, { rescan: true });
      await until(() => document.getElementById("host-tab-status")?.getAttribute("aria-selected") === "true"
        && !document.getElementById("host-tab-agents"), 10000, "revoked-host-tab-fallback");

      await say(`ok settings=machines host=agents gear=marked get=authorized cursor-connect=authorized codex-replace=manual-visible codex-copy=${copyResult} revoked=status`);
    } catch (error) {
      await say("error " + safeName(error));
    }
  })();
})();
