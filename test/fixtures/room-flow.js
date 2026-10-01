// CI only, injected only by the app built with the `probe` feature (src-tauri/src/probe.rs) when
// SIDEVOICE_DEBUG=1 SIDEVOICE_DEBUG_ROOM_FLOW=1: the native flow through the ACTUAL vendored room (ui/voice), with
// the room's own code — its controller, store, transcription client and native worker — and the app's bridge:
// capabilities → the offers the room's panes show → install a model → load it → speak → transcribe → unload it (the
// load and unload through the app's bridge, as the room's select flow calls them). Prints one line through
// debug_log: "room-flow ok …" or "room-flow error …". Nothing here is a stand-in for the room; it only drives it.
(function () {
  "use strict";
  if (location.pathname !== "/voice/index.html" && location.pathname !== "/voice/") return;
  const say = (line) => window.__TAURI_INTERNALS__.invoke("debug_log", { line: "room-flow " + line });
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  async function until(check, ms, what) {
    const end = Date.now() + ms;
    for (;;) {
      const value = check();
      if (value) return value;
      if (Date.now() > end) throw new Error("timed out waiting for " + what);
      await sleep(200);
    }
  }
  // The room's native voice worker, spoken to in its own protocol (packages/browser-audio/native-worker.js).
  function speak(worker, message) {
    return new Promise((resolve, reject) => {
      const chunks = [];
      let sampleRate = 0;
      worker.onmessage = ({ data }) => {
        if (data.type === "audio") { chunks.push(data.samples); sampleRate = data.sampleRate; }
        else if (data.type === "done") resolve({ chunks, sampleRate });
        else if (data.type === "error") reject(data.error);
      };
      worker.postMessage(message);
    });
  }
  function joined(chunks) {
    const out = new Float32Array(chunks.reduce((n, c) => n + c.length, 0));
    let at = 0;
    for (const c of chunks) { out.set(c, at); at += c.length; }
    return out;
  }
  // Whisper takes 16 kHz; the room's own capture resamples before it sends, so this does too (linear).
  function to16k(samples, rate) {
    const out = new Float32Array(Math.floor((samples.length * 16000) / rate));
    for (let i = 0; i < out.length; i++) {
      const x = (i * rate) / 16000, j = Math.floor(x), f = x - j;
      out[i] = samples[j] * (1 - f) + (samples[Math.min(j + 1, samples.length - 1)] || 0) * f;
    }
    return out;
  }
  const describe = (error) => (error && (error.key ? error.key + ": " + error.message : error.message)) || JSON.stringify(error);

  (async () => {
    try {
      await until(() => window.sidevoiceActions && window.sidevoiceUI && window.roomTranscription
        && globalThis.sidevoiceNativeWorkers && globalThis.sidevoiceNativeWorkers.available(), 60000, "the room");
      const store = window.sidevoiceUI.store;
      // 1. The room asks the app what this device is and resolves its own offers (retryGpu re-measures the device).
      await window.sidevoiceActions.retryGpu();
      const offers = await until(() => store.facts.deviceOffers && store.facts.deviceOffers.length && store.facts.deviceOffers, 30000, "offers");
      const capabilities = store.facts.deviceCapabilities;
      // 2. What the Transcripción / Voz panes show: the stage views the settings render.
      const shown = (task) => ((store.getState().stages || {})[task] || {}).models || [];
      const where = ((store.getState().stages || {}).stt || {}).where;
      const ids = (task) => offers.filter((o) => o.task === task).map((o) => o.model).join(",");
      // 3. Install through the room: its transcription client loads the native build (the worker installs it).
      const tiny = offers.find((o) => o.task === "stt" && o.model === "whisper-tiny");
      const kokoro = offers.find((o) => o.task === "tts");
      if (!tiny || !kokoro) throw new Error("no whisper-tiny or no voice offer: " + JSON.stringify(offers));
      const runtime = await window.roomTranscription.prepare({ model: tiny.model, engine: tiny.engine, accelerator: tiny.accelerator, native: true });
      // Loaded into memory before the room runs it (#124 phase 3: select = download → load → check).
      const engine = window.__sidevoiceDesktop.host.nativeEngine;
      const resident = async () => (await engine.loaded()).map((l) => l.model + "@" + l.engine + "/" + l.accelerator).sort().join(",");
      const load = await engine.load(runtime.model, runtime.engine, runtime.accelerator);
      const loaded = await resident();
      // 4. Speak with the room's native voice worker, then transcribe that with the room's transcription client.
      const voice = globalThis.sidevoiceNativeWorkers.voice();
      const started = performance.now();
      const spoken = await speak(voice, { id: 1, type: "speak", model: kokoro.model, engine: kokoro.engine,
        accelerator: kokoro.accelerator, text: "Hola, esto es una prueba de voz.", voice: "ef_dora", speed: 1 });
      const speakMs = Math.round(performance.now() - started);
      voice.terminate();
      const audio = to16k(joined(spoken.chunks), spoken.sampleRate);
      const seconds = (audio.length / 16000).toFixed(2);
      const result = await window.roomTranscription._request("transcribe", { audio: audio.buffer, model: runtime.model,
        engine: runtime.engine, accelerator: runtime.accelerator, native: true, language: "es" }, null, [audio.buffer]);
      const loadedAfter = await resident(); // the room's transcription ran on the loaded model: nothing new for it
      await engine.unload(runtime.model, runtime.engine);
      const unloaded = await resident();
      // 5. The room sees what is now on disk, and its pane marks it.
      await window.sidevoiceActions.retryGpu();
      const installed = await until(() => (store.facts.installedBuilds || []).length >= 2 && store.facts.installedBuilds, 30000, "installed");
      const tinyShown = shown("stt").find((m) => m.id === "whisper-tiny") || {};
      await say("ok capabilities=" + JSON.stringify(capabilities)
        + " engines=" + [...new Set(offers.map((o) => o.engine + "/" + o.accelerator))].join(",")
        + " offers_stt=" + ids("stt") + " offers_tts=" + ids("tts")
        + " shown_stt=" + shown("stt").map((m) => m.id).join(",") + " shown_tts=" + shown("tts").map((m) => m.id).join(",")
        + " where=" + where
        + " installed=" + installed.map((b) => b.model + "@" + b.engine).sort().join(",")
        + " tiny_detail=" + JSON.stringify(tinyShown.detail || "")
        + " runtime=" + runtime.model + "@" + runtime.engine + "/" + runtime.accelerator
        + " load_ms=" + load.load_ms + " loaded=" + loaded + " loaded_after=" + loadedAfter + " unloaded=" + unloaded
        + " audio=" + seconds + "s speak_ms=" + speakMs + " text=" + JSON.stringify(result.text));
    } catch (error) {
      await say("error " + describe(error));
    }
  })();
})();
