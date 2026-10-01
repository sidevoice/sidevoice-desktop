// CI only, injected only by the app built with the `probe` feature (src-tauri/src/probe.rs) when
// SIDEVOICE_DEBUG=1 SIDEVOICE_DEBUG_LOCAL_HOST_FLOW=1: the local host (docs/LOCAL_HOST.md) as the ACTUAL bundled page
// reaches it — the vendored room (ui/voice) at the app's own origin, with this app's bridge — against CI's stand-in
// core on ~/.sidevoice/core/local.sock (`local-host-ci core`, src-tauri/local-host/examples/local-host-ci.rs).
//
//   1. the bridge reports `running` (state() and subscribe()) and the pairing object (design §4.1);
//   2. a cross-origin fetch with the secret: WebKit's own preflight, answered by the proxy, then the request,
//      which reaches the core with the device token and the page's Origin;
//   3. without the secret: 401 the page can read; a native-only route: 404;
//   4. a WebSocket offering `sidevoice.token.<secret>`: 101, `sidevoice` agreed, a message echoed through the splice;
//      without the secret: refused.
//
// Prints one line through debug_log: "local-host-flow ok …" or "local-host-flow error …". What a page cannot send
// (a hostile, empty or `null` Origin, a wrong Host, no Origin) CI sends with curl.
(function () {
  "use strict";
  if (location.pathname !== "/voice/index.html" && location.pathname !== "/voice/") return;
  const say = (line) => window.__TAURI_INTERNALS__.invoke("debug_log", { line: "local-host-flow " + line });
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const describe = (error) => (error && (error.key ? error.key + ": " + error.message : error.message)) || String(error);

  async function socket(url, protocols) {
    return new Promise((resolve) => {
      const ws = new WebSocket(url, protocols);
      const done = (result) => { try { ws.close(); } catch (_) { /* closing */ } resolve(result); };
      const timer = setTimeout(() => done("timeout"), 15000);
      ws.onopen = () => ws.send("ping from the page");
      ws.onmessage = (event) => { clearTimeout(timer); done("open/" + ws.protocol + "/" + event.data); };
      ws.onerror = () => { clearTimeout(timer); done("refused"); };
    });
  }

  (async () => {
    const facts = [];
    const host = window.__sidevoiceDesktop && window.__sidevoiceDesktop.host;
    if (!host || !host.localHost) throw new Error("no host.localHost on this page");
    facts.push("version=" + host.version);
    const local = host.localHost;
    let state = null;
    for (let i = 0; i < 120 && (!state || state.state !== "running"); i++) {
      state = await local.state();
      if (state.state !== "running") await sleep(500);
    }
    facts.push("state=" + state.state + "/api=" + (state.core && state.core.api));
    if (state.state !== "running") throw new Error("never running: " + JSON.stringify(state));
    const subscribed = await new Promise((resolve) => {
      const stop = local.subscribe((s) => { stop(); resolve(s.state); });
    });
    facts.push("subscribed=" + subscribed);

    const pairing = await local.pairing();
    const shape = ["fp", "public_key", "device_id", "token", "urls", "rv", "host", "local"].every((k) => k in pairing);
    const base = pairing.urls[0];
    facts.push("pairing=" + (shape && pairing.local === true && pairing.rv === null && /^http:\/\/127\.0\.0\.1:\d+$/.test(base)
      && pairing.token.length === 43 ? "ok" : "bad:" + JSON.stringify(Object.keys(pairing))));
    facts.push("host=" + pairing.host);

    const echo = await fetch(base + "/api/presentation/echo", {
      method: "POST",
      headers: { Authorization: "Bearer " + pairing.token, "content-type": "application/json", accept: "application/json" },
      body: JSON.stringify({ from: "the bundled page" }),
    });
    const echoed = await echo.json();
    facts.push("fetch=" + echo.status + "/" + echoed.auth + "/" + echoed.origin + "/" + JSON.parse(echoed.body).from.replace(/ /g, "_"));
    const bare = await fetch(base + "/api/presentation/echo", { headers: { accept: "application/json" } });
    facts.push("no_secret=" + bare.status);
    const native = await fetch(base + "/api/local/health", { headers: { Authorization: "Bearer " + pairing.token } });
    facts.push("native_only=" + native.status);

    const ws = base.replace(/^http/, "ws") + "/api/browser/call";
    facts.push("ws=" + (await socket(ws, ["sidevoice", "sidevoice.token." + pairing.token])).replace(/ /g, "_"));
    facts.push("ws_no_secret=" + (await socket(ws, ["sidevoice", "sidevoice.token.guess"])));
    await say("ok " + facts.join(" "));
  })().catch((error) => say("error " + describe(error)));
})();
