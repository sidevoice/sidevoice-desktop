// Sidevoice desktop bridge — injected by the desktop app into every page its main window loads.
//
// The contract is documented in docs/BRIDGE.md. In short:
//   page → app : invoke('bridge_state', { snapshot })  whenever the call state the tray shows changes
//   app → page : window.__sidevoiceDesktop.run('toggle-mute' | 'hang-up')
// It only reads the web UI's public seams (window.sidevoiceUI.store, window.sidevoiceActions) and
// never touches the DOM. On any origin other than the window's page (the room's, or the app's own for the
// bundled interface) it does nothing at all.
(function (factory) {
  if (typeof module === "object" && module.exports) module.exports = factory; // unit tests
  else factory(window, "__SIDEVOICE_ROOM_ORIGIN__", "__SIDEVOICE_MEDIA_KEYS__");
})(function installDesktopBridge(win, roomOrigin, mediaKeys) {
  "use strict";
  // scheme://host rather than location.origin: the app's own pages (tauri://localhost) may have an opaque origin.
  if (win.location.protocol + "//" + win.location.host !== roomOrigin) return null;
  if (win.__sidevoiceDesktop) return win.__sidevoiceDesktop;

  const BRIDGE_VERSION = 1;
  let lastReported = "";

  function invoke(command, args) {
    const internals = win.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== "function") return;
    // The app may be mid-reload or the capability may not match yet: never break the page for it.
    Promise.resolve()
      .then(() => internals.invoke(command, args))
      .catch(() => {});
  }

  function call(command, args, options) {
    const internals = win.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== "function") return Promise.reject(new Error("no desktop host"));
    return Promise.resolve().then(() => internals.invoke(command, args, options));
  }

  /** Raw answers arrive as an ArrayBuffer (custom-protocol IPC) or, through the postMessage fallback, as bytes. */
  function bufferOf(answer) {
    if (Object.prototype.toString.call(answer) === "[object ArrayBuffer]") return answer;
    if (ArrayBuffer.isView(answer)) return answer.buffer.slice(answer.byteOffset, answer.byteOffset + answer.byteLength);
    if (Array.isArray(answer)) return Uint8Array.from(answer).buffer;
    throw new Error("unexpected audio from the desktop host");
  }

  let installs = 0; // numbers each install call's job

  /** The app's native model engines (docs/BRIDGE.md → "The native engine"): what this device is, which builds are on
   *  disk, get one ready, run it. A build is a catalogue model id + an engine id; the page resolves its offers itself
   *  from `capabilities()` and the catalogue. `accelerator` (optional): the one the page chose, else the app uses the
   *  resolver's first for that build here. A refusal rejects with `{key, message, …params}`: a stable key the page
   *  translates, and an English sentence for one it does not know (docs/BRIDGE.md → "Refusals"). */
  function nativeEngine() {
    return {
      /** `{runs: "native", os, arch, has: ["cpu", "coreml", …], memory_mb}` (`memory_mb` null when unknown). */
      capabilities: () => call("engine_capabilities"),
      /** `[{model, engine}]`: the builds already on disk. */
      installed: () => call("engine_installed"),
      /** Downloads the engine package (if missing) and the model's build; `onProgress(done, total)` in bytes about
       *  twice a second. */
      install(model, engine, onProgress) {
        // This call's own job: the app reports progress per job, from when it starts (after any install ahead of
        // it) until it ends — never another call's bytes.
        const job = "install-" + ++installs + "-" + Date.now().toString(36);
        const running = call("engine_install", { model, engine, job });
        if (typeof onProgress !== "function") return running;
        let finished = false;
        const poll = () => {
          if (finished) return;
          call("engine_progress", { job })
            .then((progress) => { if (!finished && Array.isArray(progress)) onProgress(progress[0], progress[1]); })
            .catch(() => {});
          win.setTimeout(poll, 500);
        };
        poll();
        return running.finally(() => { finished = true; });
      },
      /** Mono Float32Array at `sampleRate` → text. `language` empty to detect. */
      transcribe(model, engine, samples, sampleRate, language, accelerator) {
        const bytes = new Uint8Array(samples.buffer, samples.byteOffset, samples.byteLength);
        return call("engine_transcribe", bytes, {
          headers: {
            "x-model": model,
            "x-engine": engine,
            "x-accelerator": accelerator || "",
            "x-language": language || "",
            "x-sample-rate": String(sampleRate),
          },
        });
      },
      /** Text → `{samples: Float32Array, sampleRate}`. */
      async synthesize(model, engine, voice, speed, text, accelerator) {
        const args = { model, engine, accelerator: accelerator || null, voice, speed: speed || 1, text };
        const buffer = bufferOf(await call("engine_synthesize", args));
        return { sampleRate: new DataView(buffer).getUint32(0, true), samples: new Float32Array(buffer.slice(4)) };
      },
    };
  }

  /** The few facts the tray needs, read from the web UI's own view model. */
  function snapshot() {
    const store = win.sidevoiceUI && win.sidevoiceUI.store;
    const actions = win.sidevoiceActions;
    if (!store || !actions) {
      return { version: BRIDGE_VERSION, ready: false, joined: false, busy: false, micEnabled: true, micDisabled: false, title: "" };
    }
    const view = store.getState() || {};
    const call = view.call || {};
    const mic = view.mic || {};
    return {
      version: BRIDGE_VERSION,
      ready: true,
      joined: !!call.joined,
      busy: !!call.busy,
      micEnabled: mic.enabled !== false,
      micDisabled: !!mic.disabled,
      title: typeof view.title === "string" ? view.title : "",
    };
  }

  function report() {
    const state = snapshot();
    const key = JSON.stringify(state);
    if (key === lastReported) return; // the store ticks often (clocks, meters); the tray only cares about changes
    lastReported = key;
    invoke("bridge_state", { snapshot: state });
  }

  let attached = false;
  function attach() {
    if (attached) return true;
    const store = win.sidevoiceUI && win.sidevoiceUI.store;
    if (!store || typeof store.subscribe !== "function" || !win.sidevoiceActions) return false;
    store.subscribe(report);
    attached = true;
    report();
    return true;
  }

  const api = {
    version: BRIDGE_VERSION,
    /** What this host offers the page besides the webview itself. The web UI feature-detects it.
     *  `nativeEngine`: models run natively by the app (docs/ENGINES.md). */
    host: Object.freeze({
      app: "sidevoice-desktop",
      nativeEngine: Object.freeze(nativeEngine()),
      /** `"native"`: the app answers headset buttons / media keys in a call, so the page must not register its
       *  own Media Session handlers (both would toggle the microphone on one click). `null`: the page does. */
      mediaKeys: mediaKeys === "native" ? "native" : null,
    }),
    snapshot,
    /** Runs one tray/shortcut command through the web UI's own actions. Returns whether it ran. */
    run(command) {
      const actions = win.sidevoiceActions;
      const state = snapshot();
      if (!state.ready) return false;
      switch (command) {
        case "toggle-mute":
          // The web UI disables its mute button in a call with no conversation selected; so do we.
          if (state.micDisabled) return false;
          actions.toggleMic();
          report();
          return true;
        case "hang-up":
          if (!state.joined) return false;
          Promise.resolve(actions.toggleCall()).catch(() => {});
          return true;
        default:
          return false;
      }
    },
  };
  win.__sidevoiceDesktop = api;

  report(); // "not ready yet": the tray greys out until the room's controller exists
  // The room builds its store and actions after its own scripts load; wait for them.
  if (!attach()) {
    const timer = win.setInterval(() => {
      if (attach()) win.clearInterval(timer);
    }, 250);
  }
  return api;
});
