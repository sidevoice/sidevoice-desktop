// Sidevoice desktop bridge — injected by the desktop app into every page its main window loads.
//
// The contract is documented in docs/BRIDGE.md. In short:
//   page → app : invoke('bridge_state', { snapshot })  whenever the call state the app shows changes (tray, card)
//                invoke('bridge_level', { level })     the microphone's level during a call, for the card's wave
//   app → page : window.__sidevoiceDesktop.run({ command: 'toggle-mute' | 'hang-up' | 'skip-reply' | … })
// It only reads the web UI's public seams (window.sidevoiceUI.store and .micLevel, window.sidevoiceActions) and
// never touches the DOM. On any origin other than the window's page (the room's, or the app's own for the
// bundled interface) it does nothing at all.
(function (factory) {
  if (typeof module === "object" && module.exports) module.exports = factory; // unit tests
  else factory(window, "__SIDEVOICE_ROOM_ORIGIN__", "__SIDEVOICE_MEDIA_KEYS__", "__SIDEVOICE_LOCAL_HOST__");
})(function installDesktopBridge(win, roomOrigin, mediaKeys, localHostOffered) {
  "use strict";
  // scheme://host rather than location.origin: the app's own pages (tauri://localhost) may have an opaque origin.
  if (win.location.protocol + "//" + win.location.host !== roomOrigin) return null;
  if (win.__sidevoiceDesktop) return win.__sidevoiceDesktop;

  const BRIDGE_VERSION = 3;
  let lastReported = "";
  let reportedJoined = false;
  /** When the call being reported was joined (ms since the epoch), for the card's clock; null outside a call. */
  let joinedAt = null;
  /** The fields of the room's conversation rows the card shows (the web UI's ParticipantView). */
  const PARTICIPANT_FIELDS = ["threadId", "title", "selected", "available", "switching", "unread", "reach", "stateLabel",
    "activityNote", "detail", "subtitle", "working", "machine", "harness", "route"];
  const DEVICE_FIELDS = ["inputs", "outputs", "inputId", "outputId", "available", "outputAvailable", "busy"];
  const pick = (object, fields) => {
    const picked = {};
    for (const field of fields) if (object && object[field] !== undefined) picked[field] = object[field];
    return picked;
  };

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
  let hostInstalls = 0;

  // Connector progress is a process protocol. The vendored room uses stable presentation step names, so translate
  // connector-only names here instead of making the UI understand service transaction internals.
  function localInstallStep(step) {
    switch (step) {
      case "stage":
      case "wait-calls":
      case "wait-lock":
        return "staging";
      default:
        return step;
    }
  }

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
      /** Downloads the engine package (if missing) and the model's build. Returns a promise that also carries this
       *  install's job id, `promise.job`, from the start: `cancel(job)` stops it, and it then rejects with
       *  `{key: "install_cancelled", message}`. `onProgress({job, model, engine, done, total, bytes_per_s})` about
       *  twice a second once the job starts (after any install ahead of it); `bytes_per_s` is null until measured. */
      install(model, engine, onProgress) {
        // This call's own job: the app reports progress per job, from when it starts until it ends — never another
        // call's bytes.
        const job = "install-" + ++installs + "-" + Date.now().toString(36);
        const running = call("engine_install", { model, engine, job });
        let finished = false;
        const poll = () => {
          if (finished) return;
          call("engine_progress", { job })
            .then((progress) => { if (!finished && progress && typeof progress === "object") onProgress(progress); })
            .catch(() => {});
          win.setTimeout(poll, 500);
        };
        if (typeof onProgress === "function") poll();
        const promise = running.finally(() => { finished = true; });
        promise.job = job;
        return promise;
      },
      /** Cancels install `job` (`install(…).job`), waiting or running: its install rejects with
       *  `{key: "install_cancelled", message}` and the model is not on disk (unless it already was). Resolves true
       *  when the install will reject so (also when it has not reached the app yet: it is refused as it arrives);
       *  false when it had already ended. */
      cancel: (job) => call("engine_cancel", { job }),
      /** Loads the build into memory on `accelerator` (optional): `{load_ms}`. One already loaded is not loaded again
       *  (its `load_ms` is the time its load took). The app unloads it 10 minutes after its last use once no call is
       *  on, and loads it again as the next call connects (sidevoice-core#21 D13). */
      load: (model, engine, accelerator) => call("engine_load", { model, engine, accelerator: accelerator || null }),
      /** Frees the build's memory on `accelerator`, or on every accelerator when none is named; nothing to do when it
       *  is not loaded. A `load` of it still under way then rejects with `{key: "load_cancelled", …}`, and the app
       *  does not load it again as a call connects. */
      unload: (model, engine, accelerator) => call("engine_unload", { model, engine, accelerator: accelerator || null }),
      /** `[{model, engine, accelerator, since, last_used}]`: what is in memory (times in ms since the epoch). */
      loaded: () => call("engine_loaded"),
      /** `{total_mb, available_mb}` (each null when unknown). */
      memory: () => call("engine_memory"),
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

  /** This computer's own core, the local host (docs/LOCAL_HOST.md): its state, the pairing the page uses to reach it
   *  (through the app's proxy, with this launch's secret — never the device token), and the service's actions. Each
   *  action resolves the state after it (`{state, failure?, core?, service?, calls?, attempts?, limit?, reachable}`) or rejects `{key, message}`.
   *  Only where the app offers a local host (macOS). */
  function localHost() {
    const listeners = new Set();
    let last = null; // the state last delivered, as JSON
    let timer = null;
    const poll = () =>
      call("local_host_state")
        .then((state) => {
          const key = JSON.stringify(state);
          if (key === last) return;
          last = key;
          for (const listener of listeners) {
            try { listener(state); } catch (_) { /* a listener's error is the page's own */ }
          }
        })
        .catch(() => {});
    const action = (name) => () => call("local_host_action", { action: name });
    function install(onProgress) {
      const job = "local-install-" + ++hostInstalls + "-" + Date.now().toString(36);
      let sequence = 0;
      let polling = false;
      let inFlight = Promise.resolve();
      const poll = () => {
        if (typeof onProgress !== "function") return Promise.resolve();
        if (polling) return inFlight;
        polling = true;
        inFlight = call("local_host_install_progress", { job, afterSequence: sequence })
          .then((frames) => {
            if (!Array.isArray(frames)) return;
            for (const frame of frames) {
              if (!frame || !Number.isSafeInteger(frame.sequence) || frame.sequence <= sequence) continue;
              sequence = frame.sequence;
              try { onProgress({ step: localInstallStep(frame.step), done: frame.done ?? null, total: frame.total ?? null,
                cancellable: frame.cancellable === true }); }
              catch (_) { /* progress listeners cannot break the install */ }
            }
          })
          .catch(() => {})
          .finally(() => { polling = false; });
        return inFlight;
      };
      const timer = typeof onProgress === "function" ? win.setInterval(poll, 150) : null;
      const promise = call("local_host_install", { job }).finally(() => {
        if (timer !== null) win.clearInterval(timer);
        return inFlight.then(() => poll());
      });
      promise.job = job;
      return promise;
    }
    return {
      /** `{state, failure?, core?, service?, calls?, attempts?, limit?, reachable}`; `state` one of `absent`, `not-installed`, `stopped-by-person`,
       *  `starting`, `backoff`, `running`, `failed`, `service-failed`, `refused`, `incompatible`. */
      state: () => call("local_host_state"),
      /** `listener(state)` with the state now, then on every change (polled every 2 s while anyone listens).
       *  Returns `stop`. */
      subscribe(listener) {
        if (typeof listener !== "function") throw new TypeError("subscribe(listener)");
        listeners.add(listener);
        if (last !== null) {
          try { listener(JSON.parse(last)); } catch (_) { /* the page's own */ }
        }
        poll();
        if (timer === null) timer = win.setInterval(poll, 2000);
        return () => {
          listeners.delete(listener);
          if (listeners.size === 0 && timer !== null) {
            win.clearInterval(timer);
            timer = null;
            last = null;
          }
        };
      },
      /** `{fp, public_key, device_id, token, urls: ["http://127.0.0.1:<port>"], rv: null, host, local: true}`
       *  whenever a core answers and the app is paired (`reachable`), whatever `state` says; else `null`. `token` is
       *  the app's proxy secret for this launch. */
      pairing: () => call("local_host_pairing"),
      /** Capability and pinned connector metadata; this never changes the host. */
      version: () => call("local_host_version"),
      /** Explicit install only: always asks the connector to install without registering agents. */
      install,
      /** `true` only when the connector acknowledges cancellation before its commit point. */
      cancel: (job) => call("local_host_cancel", { job }),
      /** Applies an explicitly eligible bundled connector update. */
      update: () => call("local_host_update"),
      /** Agent discovery is unavailable until connector R2 provides the command. */
      agents: () => call("local_host_agents"),
      start: action("start"),
      stop: action("stop"),
      restart: action("restart"),
      serviceInstall: action("service-install"),
      serviceUninstall: action("service-uninstall"),
      /** Pairs this app with the core again (after `refused`): never done on its own. */
      reconnect: action("reconnect"),
      /** Shows the core's log in Finder (else the connector's). */
      revealLog: action("reveal-log"),
      /** A code for another device, on the person's click only: `{code, expires_in, reach}`. Shown, never sent. */
      pairingCode: () => call("local_host_pairing_code"),
      /** Pairs this machine with a room (`sidevoice pair <url> <code>`): `{room}`. */
      pairRoom: (url, code) => call("local_host_pair_room", { url, code }),
    };
  }

  /** The call as the app shows it (tray, headset, call controls card), read from the web UI's own view model. */
  function snapshot() {
    const store = win.sidevoiceUI && win.sidevoiceUI.store;
    const actions = win.sidevoiceActions;
    if (!store || !actions) {
      return { version: BRIDGE_VERSION, ready: false, joined: false, busy: false, micEnabled: true, micDisabled: false, title: "",
        agent: "idle", youTalking: false, canSkip: false, since: null, participants: [], devices: null };
    }
    const view = store.getState() || {};
    const call = view.call || {};
    const mic = view.mic || {};
    // The card's own view, projected by the room from its facts (state/room-session-state.js callCardView): the agent
    // independent of the person, and the conversation the call is on.
    const card = view.callCard || {};
    const joined = !!call.joined;
    if (!joined) joinedAt = null;
    else if (joinedAt === null) joinedAt = Date.now();
    return {
      version: BRIDGE_VERSION,
      ready: true,
      joined,
      busy: !!call.busy,
      micEnabled: mic.enabled !== false,
      micDisabled: !!mic.disabled,
      title: typeof card.title === "string" ? card.title : "",
      agent: ["idle", "working", "speaking"].includes(card.agent) ? card.agent : "idle",
      youTalking: !!card.youTalking,
      canSkip: !!card.canSkip,
      since: joinedAt,
      participants: Array.isArray(view.participants) ? view.participants.map((row) => pick(row, PARTICIPANT_FIELDS)) : [],
      devices: view.audioDevices ? pick(view.audioDevices, DEVICE_FIELDS) : null,
    };
  }

  function report() {
    const state = snapshot();
    const key = JSON.stringify(state);
    if (key === lastReported) return; // the store ticks often (clocks, meters); the tray only cares about changes
    lastReported = key;
    reportedJoined = state.joined;
    invoke("bridge_state", { snapshot: state });
  }

  // The microphone's level, for the card's wave: during a call, at most every 80 ms, and only when it changes.
  let lastLevel = -1;
  let lastLevelAt = 0;
  function reportLevel(value) {
    if (!reportedJoined) return;
    const level = Math.round(Math.max(0, Math.min(100, Number(value) || 0)));
    const now = Date.now();
    if (level === lastLevel || (level !== 0 && now - lastLevelAt < 80)) return;
    lastLevel = level;
    lastLevelAt = now;
    invoke("bridge_level", { level });
  }

  let attached = false;
  function attach() {
    if (attached) return true;
    const store = win.sidevoiceUI && win.sidevoiceUI.store;
    if (!store || typeof store.subscribe !== "function" || !win.sidevoiceActions) return false;
    store.subscribe(report);
    const level = win.sidevoiceUI.micLevel;
    if (level && typeof level.subscribe === "function") level.subscribe(reportLevel);
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
      /** The bridge's version: what the page may rely on (docs/BRIDGE.md). */
      version: BRIDGE_VERSION,
      nativeEngine: Object.freeze(nativeEngine()),
      /** `"native"`: the app answers headset buttons / media keys in a call, so the page must not register its
       *  own Media Session handlers (both would toggle the microphone on one click). `null`: the page does. */
      mediaKeys: mediaKeys === "native" ? "native" : null,
      // Only where the app offers this computer's own core (O2: macOS): the page hides what is not there.
      ...(localHostOffered === true ? { localHost: Object.freeze(localHost()) } : {}),
    }),
    snapshot,
    /** Runs one command from the app (tray, shortcut, headset, call controls card) through the web UI's own actions:
     *  `{command, …arguments}`. Returns whether it ran. */
    run(request) {
      const actions = win.sidevoiceActions;
      const state = snapshot();
      if (!state.ready || !request || typeof request !== "object") return false;
      const settle = (result) => Promise.resolve(result).catch(() => {});
      switch (request.command) {
        case "toggle-mute":
          // The web UI disables its mute button in a call with no conversation selected; so do we.
          if (state.micDisabled) return false;
          actions.toggleMic();
          report();
          return true;
        case "hang-up":
          if (!state.joined) return false;
          settle(actions.toggleCall());
          return true;
        case "skip-reply":
          if (!state.canSkip || typeof actions.skipReply !== "function") return false;
          settle(actions.skipReply());
          return true;
        case "select-participant":
          if (typeof request.threadId !== "string" || typeof actions.selectParticipant !== "function") return false;
          // Only a conversation the room lists and the call can go to; the room browses the others itself.
          if (!state.participants.some((row) => row.threadId === request.threadId && row.available !== false)) return false;
          actions.selectParticipant(request.threadId);
          return true;
        case "select-audio-device":
          if (!["input", "output"].includes(request.kind) || typeof request.id !== "string") return false;
          if (typeof actions.selectAudioDevice !== "function") return false;
          settle(actions.selectAudioDevice(request.kind, request.id));
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
