// Sidevoice desktop bridge — injected by the desktop app into every page its main window loads.
//
// The contract is documented in docs/BRIDGE.md. In short:
//   page → app : invoke('bridge_state', { snapshot })  whenever the call state the app shows changes (tray, card)
//                invoke('bridge_level', { level })     the microphone's level during a call, for the card's wave
//   app → page : window.__sidevoiceDesktop.run({ command: 'toggle-mute' | 'hang-up' | 'skip-reply' | … })
//                window.__sidevoiceDesktop.voiceEvent(event)  each event of the voice call the app runs (host.voice)
//                window.__sidevoiceDesktop.updaterEvent()     the updates' status may have changed (host.updater)
// It only reads the web UI's public seams (window.sidevoiceUI.store and .micLevel, window.sidevoiceActions) and
// never touches the DOM. On any origin other than the window's page (the room's, or the app's own for the
// bundled interface) it does nothing at all.
(function (factory) {
  if (typeof module === "object" && module.exports) module.exports = factory; // unit tests
  else factory(window, "__SIDEVOICE_ROOM_ORIGIN__", "__SIDEVOICE_MEDIA_KEYS__", "__SIDEVOICE_LOCAL_HOST__", "__SIDEVOICE_VOICE__", "__SIDEVOICE_UPDATER__");
})(function installDesktopBridge(win, roomOrigin, mediaKeys, localHostOffered, voiceOffered, updaterOffered) {
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
  /** Choices of a device the card asked for that the room has not answered yet: the card's devices are busy meanwhile.
   *  They go to the room one after another, in the order asked (the room keeps whichever answers last). */
  let choosingDevices = 0;
  let deviceChoices = Promise.resolve();
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

  /** A cancelled install, as the page reads it (`code`) and as the app says it (`key`). */
  const cancelled = () => ({ key: "install_cancelled", code: "install-cancelled", message: "The download was cancelled." });

  /** The app's model engine (docs/BRIDGE.md → "The native engine"): its catalogues and the keys of remote providers,
   *  what this device is, which builds are on disk, and get one there: its catalogues and `install` are sidevoice-web's
   *  `ModelCatalogs` as they are. A build on disk is a catalogue model id + an engine id. Running a model is the voice call's (`voice`). A refusal rejects with `{key, message, …params}`: a stable key the
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
      /** Asks `provider` with `key` without keeping it (never in the keychain, never in the listings the app keeps):
       *  what its catalogue says against it, `{code, params, detail?}` (`credential-rejected`, …; `detail` the
       *  provider's own words, without the key, at most 300 characters), or null when it lists with it.
       *  The kept key and its listing are as they were. */
      testCredential: (provider, key) =>
        coded(call("engine_test_credential", { provider, key: String(key ?? "") })).then((reason) => reason ?? null),
      /** Asks `provider` again now, past any listing kept, with the key the keychain has. */
      refresh: (provider) => coded(call("engine_refresh", { provider })).then(() => undefined),
      /** `{runs: "native", os, arch, has: ["cpu", "metal", …], memory_mb}` (`memory_mb` null when unknown). */
      capabilities: () => call("engine_capabilities"),
      /** `[{model, engine}]`: the builds already on disk. */
      installed: () => call("engine_installed"),
      /** `ModelCatalogs.install` (sidevoice-web's seam): gets a build of `model` on disk, whatever of it is missing.
       *  `options`: `build`, the build's catalogue id (`ModelBuild.id`), and/or `engine`, its backend id; with neither,
       *  the build the catalogue recommends here. `onProgress({fraction, done, total})` about twice a second once the
       *  install starts (after any install ahead of it): the share done (null while the size is unknown) and its bytes.
       *  `signal` (an AbortSignal) cancels it, waiting or running: it then rejects `{code: "install-cancelled", key:
       *  "install_cancelled", message}` — at once, without starting, when the signal is already aborted. Resolves
       *  `undefined`; any other refusal rejects `{key, code, message, …params}`. */
      install(model, options) {
        const { build = null, engine = null, onProgress, signal } = options || {};
        if (signal && signal.aborted) return Promise.reject(cancelled());
        // This call's own job: the app reports progress per job, from when it starts until it ends — never another
        // call's bytes — and cancels it by that id.
        const job = "install-" + ++installs + "-" + Date.now().toString(36);
        const running = call("engine_install", { model, build, engine, job });
        let finished = false;
        const abort = () => { if (!finished) call("engine_cancel", { job }).catch(() => {}); };
        if (signal) signal.addEventListener("abort", abort, { once: true });
        const poll = () => {
          if (finished) return;
          call("engine_progress", { job })
            .then((report) => {
              if (finished || !report || typeof report !== "object") return;
              const done = typeof report.done === "number" ? report.done : null;
              const total = typeof report.total === "number" ? report.total : null;
              onProgress({ fraction: total ? Math.min(1, (done || 0) / total) : null, done, total });
            })
            .catch(() => {});
          win.setTimeout(poll, 500);
        };
        if (typeof onProgress === "function") poll();
        return coded(running.finally(() => {
          finished = true;
          if (signal) signal.removeEventListener("abort", abort);
        })).then(() => undefined);
      },
      /** Takes `model` off this computer, every build of it; resolves `undefined` (also when none was on disk).
       *  Rejects with a refusal whose `code` the page reads: `model-in-use` while the voice call holds it,
       *  `model-installing` while an install of it is waiting or running (abort its signal first), `model-not-found`. */
      uninstall: (model) => coded(call("engine_uninstall", { model })).then(() => undefined),
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
    const listeners = { turn: new Set(), state: new Set(), level: new Set(), error: new Set(), devices: new Set() };
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
      /** The person's choices (`VoiceSettings`): each slot a model of a catalogue, `stt: {catalog, model, language,
       *  build?}`, `tts: {catalog, model, voice, speed, build?}`, with `patience`, `end_of_turn`, `idle_unload_minutes`
       *  and `turn_model?`. A local model loads the build named, else the one its catalogue picks; the app adds the voice
       *  activity detector and, unless `turn_model` names one that runs, the end-of-turn model. Rejects `{key, code,
       *  message}`: `model-unknown`, `model-unfit`, `catalog-not-found`, `build-not-found`, `vad-unavailable`,
       *  `end-of-turn-unavailable`, or the engine's own code (`credential-missing`, `memory`, ...) with the provider's
       *  `detail`. */
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
      /** The system's microphones and speakers and the ones chosen (`VoiceAudioDevices`: `{inputs, outputs, input,
       *  output, outputSelectable, lost?}`, `null` the system's default). A chosen one that went away is the default
       *  from then, said once as `lost`. */
      audioDevices: () => call("voice_audio_devices"),
      /** Chooses the microphone, the speaker or both (`{input?, output?}`: a missing kind stays, `null` is the
       *  default), mid-call too: the microphone switches at once, the speaker once what is playing has ended. Answers
       *  the devices; rejects `{key, code: "audio-device-unknown", message}` for an id not listed. */
      setAudioDevices: (choice) => {
        const chosen = {};
        for (const kind of ["input", "output"]) {
          const id = choice && choice[kind];
          if (id !== undefined) chosen[kind] = id === null ? null : String(id);
        }
        return call("voice_set_audio_devices", { choice: chosen });
      },
      /** The devices, as `audioDevices` answers them, each time they change once they have been asked for. Returns
       *  `stop`. */
      onAudioDevices: on("devices"),
    };
    return { host: Object.freeze(host), receive };
  }

  /** This computer's own core, the local host (docs/LOCAL_HOST.md): its state, the pairing the page uses to reach it
   *  (through the app's proxy, with this launch's secret — never the device token), and the service's actions. Each
   *  action resolves the state after it (`{state, failure?, core?, product_version, service?, calls?, attempts?, limit?,
   *  installed?, reachable}`) or rejects `{key, message}`.
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
      /** `{state, failure?, core?, product_version, service?, calls?, attempts?, limit?, installed?, reachable}`; `state`
       *  one of `absent`, `not-installed`, `stopped-by-person`, `starting`, `backoff`, `running`, `failed`,
       *  `service-failed`, `refused`, `incompatible`; `product_version` the installed Sidevoice's version (the
       *  `sidevoice` package, not `core.version`), `null` when unknown or not installed; `installed` whether the service
       *  starts at the next sign-in (the connector's `start_at_login`), absent when the connector does not say. */
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
      /** Whether the service starts at the next sign-in, and only that: a running service keeps running, a stopped one
       *  stays stopped (the connector's `service login on|off`). `enabled` must be a boolean; anything else rejects
       *  `{key: "bad_request"}` without running anything. A connector that cannot do it rejects
       *  `{key: "service.login-unsupported"}`. */
      setStartAtLogin: (enabled) =>
        typeof enabled === "boolean"
          ? call("local_host_action", { action: enabled ? "start-at-login-on" : "start-at-login-off" })
          : Promise.reject({ key: "bad_request", message: "setStartAtLogin(enabled) takes a boolean." }),
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

  /** The app's updates (docs/BRIDGE.md → "The updater"): sidevoice-web's `AppUpdater` seam
   *  (apps/web/src/features/updates/useUpdates.ts). The app checks the releases at launch and every 6 hours and, unless
   *  the person turned it off in the app's settings, downloads and verifies an update in the background. Only where
   *  this copy of the app can update itself (a release's .app, AppImage or Windows installer; never a .deb or a dev
   *  build). */
  function updater() {
    const listeners = new Set();
    const host = {
      /** `{available, version?}`: `available` once an update is downloaded and ready (with automatic download off, once
       *  one is found). `required` is never said. */
      status: () => call("updater_status"),
      /** `listener()` each time the status may have changed: read it again. Returns `stop`. */
      subscribe(listener) {
        if (typeof listener !== "function") throw new TypeError("subscribe(listener)");
        listeners.add(listener);
        return () => { listeners.delete(listener); };
      },
      /** Installs the update and restarts into it: at once, or (automatic download off) once it is downloaded, or once
       *  the call has ended — never mid-call. Resolves once that is under way; rejects `{key: "update_unavailable",
       *  message}` when there is nothing to install. */
      restart: () => call("updater_restart").then(() => undefined),
    };
    function changed() {
      for (const listener of listeners) {
        try { listener(); } catch (_) { /* a listener's error is the page's own */ }
      }
    }
    return { host: Object.freeze(host), changed };
  }

  /** The card's devices (the web UI's call-controls `AudioDevices`), from the room's own (`facts.audioDevices`: what the
   *  voice lists and what is chosen). The system's default comes first, as an option of its own (id `""`); a device the
   *  system lists without a name is said by its place; the speaker can be chosen only where the voice can move it. */
  function cardDevices(devices) {
    if (!devices || typeof devices !== "object") return null;
    const options = (rows) => [{ id: "", label: "", system: true }].concat((Array.isArray(rows) ? rows : [])
      .map((row, index) => row && typeof row.id === "string" && row.id
        ? (typeof row.label === "string" && row.label ? { id: row.id, label: row.label } : { id: row.id, label: "", number: index + 1 })
        : null)
      .filter(Boolean));
    const outputAvailable = !!devices.outputSelectable;
    return {
      inputs: options(devices.inputs),
      outputs: options(devices.outputs),
      inputId: typeof devices.input === "string" ? devices.input : "",
      outputId: outputAvailable && typeof devices.output === "string" ? devices.output : "",
      available: true,
      outputAvailable,
      busy: choosingDevices > 0,
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
      devices: cardDevices(view.facts && view.facts.audioDevices),
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
  // Only where this copy of the app can update itself.
  const updates = updaterOffered === true ? updater() : null;

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
      // Only where this copy of the app can update itself: the page then offers "Restart to update".
      ...(updates ? { updater: updates.host } : {}),
    }),
    /** An event of the voice call, from the app (`webview.eval`): to `host.voice`'s listeners. */
    voiceEvent(event) {
      if (voiceCall) voiceCall.receive(event);
    },
    /** The updates' status may have changed, from the app (`webview.eval`): `host.updater.subscribe`'s listeners. */
    updaterEvent() {
      if (updates) updates.changed();
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
          if (typeof actions.setAudioDevices !== "function") return false;
          // The room's own choice (`setAudioDevices`), kept for this device as its device bar keeps it; `""` is the
          // system's default, which the room chooses as `null`. The card's devices are busy until the room has answered
          // every choice, each asked once the one before has been answered: the last asked is the one that stays.
          choosingDevices += 1;
          report();
          deviceChoices = deviceChoices
            .then(() => actions.setAudioDevices({ [request.kind]: request.id === "" ? null : request.id }))
            .catch(() => {})
            .finally(() => {
              choosingDevices -= 1;
              report();
            });
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
