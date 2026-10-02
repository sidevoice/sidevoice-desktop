// CI only, injected only by the app built with the `probe` feature (src-tauri/src/probe.rs) when
// SIDEVOICE_DEBUG=1 SIDEVOICE_DEBUG_ROOM_FLOW=1: model selection (sidevoice/sidevoice-core#21 §6, D11–D12) through the
// ACTUAL vendored room (ui/voice) — its settings actions, the ones its Transcripción pane calls, its controller,
// selection state machine, native worker and storage — over this app's bridge and native engine, with real models.
// Nothing here stands in for the room: it pairs the device with CI's stand-in machine (test/fixtures/fake-node.py,
// whose key it pins) so the room stores its choices, then only calls the room's actions and reads what they did.
//
//   1. consent → download → load → check twice → in effect, stored            (whisper-base)
//   2. cancel mid-download: nothing stored, loaded or left installed           (whisper-small)
//   3. a failure (the load, refused: SIDEVOICE_DEBUG_REFUSE_LOAD) rolls back   (whisper-tiny)
//   4. slow (SIDEVOICE_DEBUG_SLOW_TRANSCRIBE): "elegir otro" keeps the old one, "usar igualmente" takes the new one
//      — and only one copy of the model stays in memory either way             (whisper-base on Core ML, Avanzado)
//   5. the page reloads: settings show the stored choice, and the next selection starts from it.
//
// Prints one line through debug_log: "room-flow ok …" or "room-flow error …".
(function () {
  "use strict";
  if (location.pathname !== "/voice/index.html" && location.pathname !== "/voice/") return;
  const FLOW = "sidevoice-room-flow";
  // CI's stand-in machine (test/fixtures/fake-node.py --key): a throwaway key that signs nothing but CI's nonces.
  const MACHINE = "XO8Z6hj1_KrQXjxM2FXpBPd6qOyC__Dfc4zLn-4dOwI";
  const MACHINE_KEY = "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEe+HC+jHE39mN/AXjcJyjxUr/FeJ335haZ4Kgjoj6ZhZm90SXgy1EH2nNFOxZTXr4P892sl5p+eiDv8sLhAOywg==";
  const part = sessionStorage.getItem(FLOW) || "select";
  if (part === "select") {
    // A device paired with that machine and nothing chosen yet; set before the room's own scripts read storage.
    localStorage.removeItem("sidevoice.stages");
    localStorage.removeItem("sidevoice.settings");
    localStorage.setItem("sidevoice.pairings", JSON.stringify({ in_use: MACHINE, pairings: [{ fp: MACHINE, token: "ci-token",
      public_key: MACHINE_KEY, device_id: "ci-device", urls: ["http://127.0.0.1:8768"], rv: null, host: "ci-runner", paired_at: Date.now() }] }));
  }

  const say = (line) => window.__TAURI_INTERNALS__.invoke("debug_log", { line: "room-flow " + line });
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  async function until(check, ms, what) {
    const end = Date.now() + ms;
    for (;;) {
      const value = await check();
      if (value) return value;
      if (Date.now() > end) throw new Error("timed out waiting for " + what);
      await sleep(100);
    }
  }
  const describe = (error) => (error && (error.key ? error.key + ": " + error.message : error.message)) || JSON.stringify(error);
  const store = () => window.sidevoiceUI.store;
  const facts = () => store().getState().facts;
  const check = (task) => (facts().stageChecks || {})[task] || null;
  /** The selection of `task` at one of `phases`; a failure nobody waits for ends the flow with its step and reason.
   *  What the pane said before the action (`stale`, the last selection's outcome) is not an answer to it: it stays
   *  until the new selection publishes its first step. */
  const reach = (task, phases, ms, stale) => until(() => {
    const now = check(task);
    if (now && now === stale) return false;
    if (now && now.phase === "failed" && !phases.includes("failed"))
      throw new Error("selection failed at " + now.step + ": " + JSON.stringify(now.reason));
    return now && phases.includes(now.phase) && now;
  }, ms, task + " " + phases.join("/"));
  const stored = () => {
    const scope = JSON.parse(localStorage.getItem("sidevoice.stages") || "{}");
    const stt = (scope.hosts && scope.hosts[MACHINE] || {}).stt;
    return stt ? stt.model + "/" + ((stt.build && stt.build.accelerator) || "auto") : "none";
  };
  const engine = () => window.__sidevoiceDesktop.host.nativeEngine;
  const resident = async () => (await engine().loaded()).map((l) => l.model + "@" + l.engine + "/" + l.accelerator).sort().join(",");
  const settles = (want, what) => until(async () => (await resident()) === want, 15000, what + " = " + want);
  const room = () => until(() => window.sidevoiceActions && window.sidevoiceUI && window.roomTranscription
    && globalThis.sidevoiceNativeWorkers && globalThis.sidevoiceNativeWorkers.available(), 60000, "the room");

  async function select() {
    await room();
    const actions = window.sidevoiceActions;
    const out = {};
    // What the room makes of this device: the app's capabilities, and the offers its panes show.
    await actions.retryGpu();
    const offers = await until(() => (facts().deviceOffers || []).length && facts().deviceOffers, 30000, "offers");
    const shown = (task) => ((store().getState().stages || {})[task] || {}).models || [];
    const ids = (task) => offers.filter((o) => o.task === task).map((o) => o.model).join(",");
    out.capabilities = JSON.stringify(facts().deviceCapabilities);
    out.engines = [...new Set(offers.map((o) => o.engine + "/" + o.accelerator))].join(",");
    out.offers_stt = ids("stt");
    out.offers_tts = ids("tts");
    out.shown_stt = shown("stt").map((m) => m.id).join(",");
    out.shown_tts = shown("tts").map((m) => m.id).join(",");
    out.where = ((store().getState().stages || {}).stt || {}).where;

    // 1. A model not on disk: its size asked for, then downloaded, loaded, checked twice, in effect and stored.
    let stale = check("stt");
    actions.chooseStageModel("stt", "whisper-base");
    const consent = await reach("stt", ["consent"], 30000, stale);
    out.consent_size = consent.size;
    out.stored_before = stored();
    actions.decideStage("stt", true);
    await reach("stt", ["done"], 300000);
    const diagnostics = (facts().stageDiagnostics || {}).stt || {};
    out.chosen = stored();
    out.chosen_resident = await resident();
    out.chosen_passes = (diagnostics.passes || []).length;
    out.chosen_latency_ms = (diagnostics.passes || []).map((p) => p.latency_ms).join("/");
    out.chosen_load_ms = diagnostics.load_ms;

    // 2. Cancel mid-download: the selection stops, nothing is stored, the app's install is cancelled, nothing loads.
    stale = check("stt");
    actions.chooseStageModel("stt", "whisper-small");
    await reach("stt", ["consent"], 30000, stale);
    actions.decideStage("stt", true);
    const downloading = await until(() => {
      const now = check("stt");
      return now && now.phase === "running" && now.progress && now.progress.step === "download" && now.progress.done > 0 && now;
    }, 120000, "whisper-small downloading");
    out.cancel_at = downloading.progress.done + "/" + downloading.progress.total;
    actions.cancelStage("stt");
    await until(() => check("stt") === null, 30000, "the selection to end");
    await sleep(3000); // a late load would show by now (review R04)
    const row = (facts().downloads || []).find((d) => String(d.id).includes("whisper-small")) || {};
    out.cancel_download = row.state || "none";
    out.cancel_stored = stored();
    out.cancel_resident = await resident();
    out.cancel_installed = (await engine().installed()).some((b) => b.model === "whisper-small");

    // 3. A model that fails (its load refused): the step and cause shown, the one in use stays in use and loaded.
    stale = check("stt");
    actions.chooseStageModel("stt", "whisper-tiny");
    await reach("stt", ["consent"], 30000, stale);
    actions.decideStage("stt", true);
    const failed = await reach("stt", ["failed"], 300000);
    out.fail_step = failed.step;
    out.fail_key = failed.reason && failed.reason.key;
    out.fail_stored = stored();
    out.fail_resident = await resident();

    // 4. Avanzado → Core ML, which checks slow: "elegir otro" leaves everything as it was, the candidate's copy
    // freed; "usar igualmente" puts it in effect and frees the copy it replaced (review R05).
    stale = check("stt");
    actions.chooseStageBuild("stt", "sherpa-onnx/coreml");
    const slow = await reach("stt", ["slow"], 300000, stale);
    out.slow_latency_ms = slow.result && slow.result.latency_ms;
    actions.decideStage("stt", false);
    await until(() => check("stt") === null, 30000, "the slow one declined");
    out.declined_stored = stored();
    await settles("whisper-base@sherpa-onnx/cpu", "declined");
    out.declined_resident = await resident();
    stale = check("stt");
    actions.chooseStageBuild("stt", "sherpa-onnx/coreml");
    await reach("stt", ["slow"], 300000, stale);
    actions.decideStage("stt", true);
    await reach("stt", ["done"], 60000);
    out.accepted_stored = stored();
    await settles("whisper-base@sherpa-onnx/coreml", "accepted");
    out.accepted_resident = await resident();
    return out;
  }

  // 5. After a reload: settings read the choice back from storage, and the next selection starts from it.
  async function reloaded() {
    await room();
    const actions = window.sidevoiceActions;
    const out = {};
    try {
      await until(() => facts().nodeReach === "ok", 60000, "the machine");
    } catch (error) {
      const current = facts();
      const remote = current.remoteHostStatus && current.remoteHostStatus[MACHINE];
      throw new Error(error.message + " (reach=" + (current.nodeReach || "none")
        + " selected=" + (current.pairingInUse || "none") + " node=" + (current.node || "none")
        + " remote=" + (remote && remote.state || "none")
        + " local_selected=" + (current.localHostSelected === true)
        + " local=" + (current.localHostStatus && current.localHostStatus.state || "none") + ")");
    }
    out.reach = facts().nodeReach;
    document.getElementById("settings-open").click();
    const preferences = await until(() => facts().voicePreferences && facts().voicePreferences.stt, 30000, "the settings");
    out.restored = preferences.model + "/" + ((preferences.build && preferences.build.accelerator) || "auto");
    // The pane draws the build once the room has measured this device again (after the reload): wait for it.
    const pane = await until(() => {
      const stt = (store().getState().stages || {}).stt;
      return stt && stt.model && stt.advanced && stt.advanced.value && stt;
    }, 30000, "the pane's build");
    out.restored_pane = pane.model + "/" + pane.advanced.value;
    const stale = check("stt");
    actions.chooseStageBuild("stt", "auto");
    await reach("stt", ["done"], 120000, stale);
    out.after_stored = stored();
    await settles("whisper-base@sherpa-onnx/cpu", "after the reload");
    out.after_resident = await resident();
    return out;
  }

  const line = (out) => Object.entries(out).map(([key, value]) => key + "=" + (typeof value === "string" ? value : JSON.stringify(value))).join(" ");
  (async () => {
    try {
      if (part === "select") {
        const out = await select();
        sessionStorage.setItem(FLOW, "reloaded");
        sessionStorage.setItem(FLOW + "-select", line(out));
        await say("selected " + line(out));
        location.reload();
        return;
      }
      const out = await reloaded();
      await say("ok " + sessionStorage.getItem(FLOW + "-select") + " reload_" + line(out).replace(/ (?=[a-z_]+=)/g, " reload_"));
    } catch (error) {
      await say("error (" + part + ") " + describe(error));
    } finally {
      if (part !== "select") { sessionStorage.removeItem(FLOW); sessionStorage.removeItem(FLOW + "-select"); }
    }
  })();
})();
