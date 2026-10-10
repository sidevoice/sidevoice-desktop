// Sidevoice desktop bridge — injected by the desktop app into every page its main window loads.
//
// The contract is documented in docs/BRIDGE.md. In short:
//   page → app : invoke('bridge_state', { snapshot })  whenever the call state the app shows changes (tray, card)
//                invoke('bridge_level', { level })     the microphone's level during a call, for the card's wave
//   app → page : window.__sidevoiceDesktop.run({ command: 'toggle-mute' | 'hang-up' | 'skip-reply' | … })
//                window.__sidevoiceDesktop.voiceEvent(event)  each event of the voice call the app runs (host.voice)
// It only reads the web UI's public seams (window.sidevoiceUI.store and .micLevel, window.sidevoiceActions) and
// never touches the DOM. On any origin other than the window's page (the room's, or the app's own for the
// bundled interface) it does nothing at all.
(function (factory) {
  if (typeof module === "object" && module.exports) module.exports = factory; // unit tests
  else factory(window, "__SIDEVOICE_ROOM_ORIGIN__", "__SIDEVOICE_MEDIA_KEYS__", "__SIDEVOICE_LOCAL_HOST__", "__SIDEVOICE_VOICE__");
})(function installDesktopBridge(win, roomOrigin, mediaKeys, localHostOffered, voiceOffered) {
  "use strict";
  // scheme://host rather than location.origin: the app's own pages (tauri://localhost) may have an opaque origin.
  if (win.location.protocol + "//" + win.location.host !== roomOrigin) return null;
  if (win.__sidevoiceDesktop) return win.__sidevoiceDesktop;

  const BRIDGE_VERSION = 2;
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

  let installs = 0; // numbers each install call's job

  /** A refusal of `host.engine`'s catalogue and key calls, with the `code` the page reads: the refusal's own, else its
   *  key with dashes. */
  function coded(promise) {
    return promise.catch((refusal) => {
      const key = refusal && typeof refusal === "object" ? refusal.key : undefined;
      throw Object.assign({}, refusal, { code: refusal?.code ?? String(key ?? "internal").replace(/_/g, "-") });
    });
  }

  /** The app's model engine (docs/BRIDGE.md → "The native engine"): its catalogues and the keys of remote providers,
   *  what this device is, which builds are on disk, and get one there. A build is a catalogue model id + an engine id.
   *  Running a model is the voice call's (`voice`). A refusal rejects with `{key, message, …params}`: a stable key the
   *  page translates, and an English sentence for one it does not know (docs/BRIDGE.md → "Refusals"). */
  function engine() {
    return {
      /** `[{id, name, status: {reason?, stale, detail?}, models}]`: every catalogue, the local one first (`name` null),
       *  then each provider's, each with every model it lists in `/engine`'s shape (a local model with its
       *  family and builds). One that cannot list (a provider with no key) has `models: []` and says why in `status`.
       *  Rejects `{code}` only when the engine itself fails. */
      catalogs: () => coded(call("engine_catalogs")),
      /** Keeps `key` for remote catalogue `provider` in the keychain, or removes it (`null` or blank); that provider is
       *  then listed again with it. */
      setCredential: (provider, key) =>
        coded(call("engine_set_credential", { provider, key: key == null ? null : String(key) })).then(() => undefined),
      /** Whether the keychain holds a key for `provider`. The key itself never reaches the page. */
      hasCredential: (provider) => coded(call("engine_has_credential", { provider })),
      /** `{runs: "native", os, arch, has: ["cpu", "metal", …], memory_mb}` (`memory_mb` null when unknown). */
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
      /** `{total_mb, available_mb}` (each null when unknown). */
      memory: () => call("engine_memory"),
    };
  }

  /** The voice call the app runs itself (docs/BRIDGE.md → "The voice call"): sidevoice-voice natively, with the
   *  device's microphone and speaker and the app's own echo cancellation. It implements `VoiceHost`, the seam
   *  `@sidevoice/voice` defines (js/voice-host.d.ts), which `createVoiceHost` implements in a browser. The call knows
   *  nothing of the room: the page tells the room the turns `onTurn` gives it, and has the call `say` what the room
   *  sends, through a handle that tells how it went. A refusal rejects with `{key, message, …params}`. Only where the
   *  app offers it (macOS); elsewhere the page runs the call itself. */
  function voice() {
    const listeners = { turn: new Set(), state: new Set(), level: new Set(), error: new Set() };
    const on = (kind) => (listener) => {
      if (typeof listener !== "function") throw new TypeError("a listener");
      listeners[kind].add(listener);
      return () => listeners[kind].delete(listener);
    };
    // What is being said, by the key this bridge gave it, until its `done` step.
    const sayings = new Map();
    let said = 0;
    /** One event of the call, sidevoice-voice's `VoiceEvent` as JSON (`{type, data}`), to its listeners; a step of
     *  something said (`{type: "say", data: {key, event}}`) to its handle. */
    function receive(event) {
      if (!event || typeof event !== "object") return;
      if (event.type === "say") {
        const saying = event.data && sayings.get(event.data.key);
        if (saying) saying.step(event.data.event);
        return;
      }
      for (const listener of listeners[event.type] || []) {
        try { listener(event.data); } catch (_) { /* a listener's error is the page's own */ }
      }
    }
    /** A handle (`VoiceSaying`) for `text` said under `key`: its steps reach the listeners subscribed when each comes,
     *  `done` settles `outcome`, and `cancel` goes behind the `say` itself, so it never overtakes it. */
    function saying(key, text, options) {
      const heard = new Set();
      let settle;
      const outcome = new Promise((resolve) => { settle = resolve; });
      const asked = call("voice_say", { key, text: String(text), language: (options && options.language) ?? null })
        .catch((error) => step({ type: "done", outcome: { status: "not-played", reason: "failed", code: (error && (error.code || error.key)) || "voice-failed" } }));
      function step(event) {
        for (const listener of heard) {
          try { listener(event); } catch (_) { /* a listener's error is the page's own */ }
        }
        if (event && event.type === "done") {
          sayings.delete(key);
          settle(event.outcome);
        }
      }
      const handle = Object.freeze({
        id: key,
        cancel: () => { asked.then(() => sayings.has(key) && call("voice_cancel_say", { key })).catch(() => {}); },
        onEvent(listener) {
          if (typeof listener !== "function") throw new TypeError("a listener");
          heard.add(listener);
        },
        outcome,
      });
      sayings.set(key, { step });
      return handle;
    }
    const host = {
      /** The person's choices (`VoiceSettings`): each slot a model of a catalogue, `stt: {catalog, model, language}`,
       *  `tts: {catalog, model, voice, speed}`, with `patience`, `end_of_turn` and `idle_unload_minutes`. The catalogue
       *  picks the build; the app adds the voice activity detector and the end-of-turn model. Rejects `{key, code,
       *  message}`: `model-unknown`, `model-unfit`, `catalog-not-found`, `vad-unavailable`, `end-of-turn-unavailable`,
       *  or the engine's own code (`credential-missing`, ...) with the provider's `detail`. */
      setSettings: (settings) => call("voice_set_settings", { settings }),
      /** Loads the models (installing them if they are not), opens the microphone and the speaker and listens.
       *  Resolves once it listens (at once if it does); rejects `{key, code, message}`, `code` the call's or `stopped`. */
      start: () => call("voice_start"),
      /** Stops listening and speaking (the turn not told yet is cancelled, what is being said ends `stopped`); the
       *  models stay loaded. */
      stop: () => call("voice_stop"),
      /** Says `text` (`options`: `{language?}`) after whatever is being said, and answers its handle at once. */
      say: (text, options) => saying("say-" + ++said, text, options),
      mute: (muted) => call("voice_mute", { muted: !!muted }),
      cancelInput: () => call("voice_cancel_input"),
      /** A turn of the person's under the call's own `turn_id`: `started`, then `finished` (the words) or
       *  `cancelled`. Returns `stop`. */
      onTurn: on("turn"),
      /** `{listening, recognising, playback}` when it changes. Returns `stop`. */
      onState: on("state"),
      /** The microphone's level, 0 to 1, about 30 times a second. Returns `stop`. */
      onLevel: on("level"),
      /** `{code}`: something failed that the person may be told. Returns `stop`. */
      onError: on("error"),
    };
    return { host: Object.freeze(host), receive };
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

  // Only where the app runs the voice call itself (macOS): elsewhere the page runs it.
  const voiceCall = voiceOffered === true ? voice() : null;

  const api = {
    version: BRIDGE_VERSION,
    /** What this host offers the page besides the webview itself. The web UI feature-detects it.
     *  `engine`: the app's engine, its catalogues, keys and models on disk (docs/ENGINES.md); `voice`: the call the
     *  app runs. */
    host: Object.freeze({
      app: "sidevoice-desktop",
      /** The bridge's version: what the page may rely on (docs/BRIDGE.md). */
      version: BRIDGE_VERSION,
      engine: Object.freeze(engine()),
      /** `"native"`: the app answers headset buttons / media keys in a call, so the page must not register its
       *  own Media Session handlers (both would toggle the microphone on one click). `null`: the page does. */
      mediaKeys: mediaKeys === "native" ? "native" : null,
      // Only where the app offers this computer's own core (O2: macOS): the page hides what is not there.
      ...(localHostOffered === true ? { localHost: Object.freeze(localHost()) } : {}),
      // Only where the app runs the voice call (macOS): the page then neither opens the microphone nor runs models.
      ...(voiceCall ? { voice: voiceCall.host } : {}),
    }),
    /** An event of the voice call, from the app (`webview.eval`): to `host.voice`'s listeners. */
    voiceEvent(event) {
      if (voiceCall) voiceCall.receive(event);
    },
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
