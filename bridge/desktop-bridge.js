// Sidevoice desktop bridge — injected by the desktop app into every page its main window loads.
//
// The contract is documented in docs/BRIDGE.md. In short:
//   page → app : invoke('bridge_state', { snapshot })  whenever the call state the app shows changes (tray, card)
//                invoke('bridge_level', { level })     the microphone's level during a call, for the card's wave
//                invoke('bridge_playback', { level, threadId })  the voice's output level, for the card's avatar
//   app → page : window.__sidevoiceDesktop.run({ command: 'toggle-mute' | 'hang-up' | 'skip-reply' | … })
//                window.__sidevoiceDesktop.voiceEvent(event)  each event of the voice call the app runs (host.voice)
//                window.__sidevoiceDesktop.updaterEvent(status)  the updates' status changed (host.updater)
//                window.__sidevoiceDesktop.muteShortcutEvent(status)  the mute shortcut changed (host.muteShortcut)
// It only reads the web UI's public seams (window.sidevoiceUI.store, .micLevel, .playbackLevel and .openSettings,
// window.sidevoiceActions) and
// never touches the DOM. On any origin other than the window's page (the room's, or the app's own for the
// bundled interface) it does nothing at all.
(function (factory) {
  if (typeof module === "object" && module.exports) module.exports = factory; // unit tests
  else factory(window, "__SIDEVOICE_ROOM_ORIGIN__", "__SIDEVOICE_MEDIA_KEYS__", "__SIDEVOICE_LOCAL_HOST__", "__SIDEVOICE_VOICE__", "__SIDEVOICE_UPDATER__", "__SIDEVOICE_BUILD__", "__SIDEVOICE_COMPUTER_NAME__", "__SIDEVOICE_DEBUG__");
})(function installDesktopBridge(win, roomOrigin, mediaKeys, localHostOffered, voiceOffered, updaterOffered, build, computerName, debugState) {
  "use strict";
  // scheme://host rather than location.origin: the app's own pages (tauri://localhost) may have an opaque origin.
  if (win.location.protocol + "//" + win.location.host !== roomOrigin) return null;
  // The app never tells the interface where to look first (sidevoice-web's `__SIDEVOICE_TARGET__`): pairing does.
  win.__SIDEVOICE_TARGET__ = null;
  if (win.__sidevoiceDesktop) return win.__sidevoiceDesktop;

  const BRIDGE_VERSION = 2;
  let lastReported = "";
  let reportedJoined = false;
  /** When the call being reported was joined (ms since the epoch), for the card's clock; null outside a call. */
  let joinedAt = null;
  /** The fields of the room's conversation rows the card shows (the web UI's ParticipantView). */
  const PARTICIPANT_FIELDS = ["threadId", "title", "selected", "available", "switching", "unread", "reach", "stateLabel",
    "activityNote", "detail", "subtitle", "working", "machine", "harness", "route", "machineReachable"];
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
  /** Native implementation of sidevoice-voice's VoiceHost. The host owns one microphone, speaker and model pool;
   *  `start(settings)` makes a call with an input half, an output half or both, which owns its events, playback and
   *  recognition state, and `preload(settings)` loads the models a call of them would use. */
  function voice() {
    const calls = new Map();
    const deviceListeners = new Set();
    const epoch = Math.random().toString(36).slice(2, 10);
    let nextCall = 0;
    let route = Promise.resolve();
    const on = (set) => (listener) => {
      if (typeof listener !== "function") throw new TypeError("a listener");
      set.add(listener);
      return () => set.delete(listener);
    };
    const dispatch = (set, data) => {
      for (const listener of set) {
        try { listener(data); } catch (_) { /* a listener's error is the page's own */ }
      }
    };
    function takeOf(data) {
      const binary = win.atob(String(data.samples || ""));
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
      return { ...data, samples: new Float32Array(bytes.buffer, 0, bytes.length >> 2) };
    }
    function sayStep(event) {
      if (event?.type !== "done" || !event.outcome || typeof event.outcome.reason !== "object") return event;
      const { reason, ...outcome } = event.outcome;
      return { ...event, outcome: { ...outcome, ...reason } };
    }
    const needsAHalf = () => new TypeError("voice settings with an input or an output half");
    function start(settings, options) {
      if (!settings || typeof settings !== "object" || (!settings.input && !settings.output)) throw needsAHalf();
      const id = `call-${epoch}-${++nextCall}`;
      const listeners = {
        turn: new Set(), state: new Set(), level: new Set(), "playback-level": new Set(), error: new Set(), take: new Set(),
      };
      const sayings = new Map();
      let said = 0;
      // Each stop waits for the idle state that answers it, which the take and the end of an open turn come before.
      let stopping = 0;
      let answers = [];
      // Mutes and turn ends reach the app in the order they were asked; each settles quietly, as the browser's `mute`
      // returns nothing. Changes of the call's settings are made one after the other too.
      let muting = Promise.resolve();
      let updating = Promise.resolve();
      // Ending: nothing new is asked, but the call's own ends still come in. Ended: nothing more is delivered.
      let closing = false;
      let disposed = false;
      let ending = null;
      const closed = () => Object.assign(new Error("call stopped"), { code: "stopped", closed: true });
      // The lane is made at once, so that the microphone may be routed to the call before it works; the call is made on
      // it (the app refuses here what its catalogues cannot fill); `ready` is when it works.
      const created = call("voice_create_call", { key: id });
      const installed = created.then(() => call("voice_start", { key: id, settings, take: options?.take === true }));
      // It works when the app says so and the page has the state that says it (the app's events and its answers reach the
      // page by different ways, and a page that hears `ready` must find the state it comes with).
      let sawWorking;
      let failWorking;
      const working = new Promise((resolve, reject) => { sawWorking = resolve; failWorking = reject; });
      working.catch(() => {});
      const ready = installed.then(() => call("voice_ready", { key: id })).then(() => working);
      // The page reads `ready`; one it never reads is no unhandled rejection of the bridge's.
      ready.catch(() => {});
      const asked = (after) => (command, args) => after.then(() => {
        if (closing) throw closed();
        return call(command, { key: id, ...args });
      });
      // What needs the lane only (the app keeps a mute from the lane's creation), and what needs the call.
      const onLane = asked(created);
      const onCall = asked(installed);
      function saying(text, options) {
        const key = `say-${++said}`;
        const heard = new Set();
        let settle;
        const outcome = new Promise((resolve) => { settle = resolve; });
        function step(event) {
          if (!sayings.has(key)) return;
          dispatch(heard, event);
          if (event?.type === "done") { sayings.delete(key); settle(event.outcome); }
        }
        sayings.set(key, { step, outcome });
        const asking = onCall("voice_say", { sayKey: key, text: String(text), language: options?.language ?? null })
          .catch((error) => step({ type: "done", outcome: error?.closed
            ? { status: "not-played", reason: "stopped" }
            : { status: "not-played", reason: "failed", code: error?.code || error?.key || "voice-failed" } }));
        return Object.freeze({
          id: key,
          cancel: () => { asking.then(() => sayings.has(key) && call("voice_cancel_say", { key: id, sayKey: key })).catch(() => {}); },
          onEvent(listener) { if (typeof listener !== "function") throw new TypeError("a listener"); heard.add(listener); },
          outcome,
        });
      }
      // Stops the call's listening and speaking, answering once its idle state came with what the stop ends.
      function halt() {
        let answer;
        const answered = new Promise((resolve) => { answer = resolve; });
        stopping++;
        answers.push(answer);
        const forget = () => {
          const at = answers.indexOf(answer);
          if (at >= 0) { answers.splice(at, 1); stopping--; }
        };
        // Native answers true once it stopped a call; where none was made, no idle comes.
        return call("voice_stop", { key: id }).then((ran) => {
          if (ran === true) return answered;
          forget();
          return undefined;
        }, (error) => { forget(); throw error; });
      }
      function stop() {
        if (ending) return ending;
        closing = true;
        ending = (async () => {
          await route.catch(() => {});
          // The call is made, or the app refused it: nothing to stop then, and no lane to release when there is none.
          await installed.catch(() => {});
          if (!await created.then(() => true, () => false)) return;
          await halt().catch(() => {});
          // Stopped, each saying under way ends as the call says it did.
          await Promise.all([...sayings.values()].map((pending) => pending.outcome));
          await call("voice_dispose_call", { key: id });
        })().finally(() => {
          disposed = true;
          failWorking(closed());
          calls.delete(id);
          for (const set of Object.values(listeners)) set.clear();
          for (const resolve of answers) resolve();
          answers = [];
        });
        return ending;
      }
      const host = Object.freeze({
        id,
        ready,
        update: (change) => {
          if (!change || typeof change !== "object") return Promise.reject(new TypeError("a change of voice settings"));
          updating = updating.catch(() => {}).then(() => onCall("voice_update", { change }));
          return updating;
        },
        stop,
        say: saying,
        mute: (muted) => {
          muting = muting.then(() => onLane("voice_mute", { muted: !!muted })).catch(() => {});
        },
        cancelInput: () => { onLane("voice_cancel_input").catch(() => {}); },
        // In order with mutes; resolves once native closed the turn, or at once when there is nothing to close.
        endInput: () => {
          const ended = muting.then(() => onLane("voice_end_input"));
          muting = ended.then(() => undefined, () => undefined);
          return muting;
        },
        onTurn: on(listeners.turn),
        onState: on(listeners.state),
        onLevel: on(listeners.level),
        onPlaybackLevel: on(listeners["playback-level"]),
        onError: on(listeners.error),
        onTake: on(listeners.take),
      });
      calls.set(id, { host, created, listeners, sayings, receive(event) {
        if (disposed) return;
        if (event.type === "say") {
          sayings.get(event.data?.key)?.step(sayStep(event.data.event));
          return;
        }
        const data = event.type === "take" ? takeOf(event.data) : event.data;
        if (listeners[event.type]) dispatch(listeners[event.type], data);
        if (event.type === "state" && data?.listening !== "idle") sawWorking();
        if (event.type === "state" && data?.listening === "idle" && stopping > 0) {
          stopping--;
          answers.shift()?.();
        }
      } });
      return host;
    }
    const host = Object.freeze({
      start,
      preload(settings) {
        if (!settings || typeof settings !== "object" || (!settings.input && !settings.output)) {
          return Promise.reject(needsAHalf());
        }
        return call("voice_preload", { settings });
      },
      routeInput(target) {
        if (target !== null && (!target || calls.get(target.id)?.host !== target)) {
          return Promise.reject(new TypeError("a call from this voice host or null"));
        }
        route = route.catch(() => {}).then(async () => {
          if (target) await calls.get(target.id).created;
          return call("voice_route_input", { key: target?.id ?? null });
        });
        return route;
      },
      audioDevices: () => call("voice_audio_devices"),
      setAudioDevices: (choice) => {
        const chosen = {};
        for (const kind of ["input", "output"]) {
          const id = choice && choice[kind];
          if (id !== undefined) chosen[kind] = id === null ? null : String(id);
        }
        return call("voice_set_audio_devices", { choice: chosen });
      },
      onAudioDevices: on(deviceListeners),
    });
    return { host, receive(event) {
      if (!event || typeof event !== "object") return;
      if (event.type === "devices") dispatch(deviceListeners, event.data);
      else calls.get(event.key)?.receive(event);
    } };
  }

  /** The app's microphone permission (docs/BRIDGE.md → "The microphone permission"), onto the `microphone` command:
   *  where the app runs the voice call, the page is never granted `getUserMedia`, so it can neither ask macOS for the
   *  microphone nor read whether it may. The first-run setup asks through this. Only with `voice` (macOS). */
  function microphone() {
    const listeners = new Set();
    let timer = null;
    let last = null;
    const poll = () =>
      call("microphone", { action: "state" })
        .then((state) => {
          if (last !== null && state !== last) {
            for (const listener of [...listeners]) {
              try { listener(); } catch (_) { /* a listener's error is the page's own */ }
            }
          }
          last = state;
        })
        .catch(() => {});
    return Object.freeze({
      /** `"prompt"` (not asked yet), `"granted"`, `"denied"` (denied, or forbidden by the system) or `"missing"` (not
       *  denied, and no audio input device exists). Never prompts. */
      state: () => call("microphone", { action: "state" }),
      /** macOS's own prompt the first time; afterwards the stored answer at once, with no prompt: `"granted"` or
       *  `"denied"`. */
      request: () => call("microphone", { action: "request" }),
      /** Opens System Settings at Privacy & Security › Microphone, where a denial is undone. */
      openSettings: () => call("microphone", { action: "open-settings" }).then(() => undefined),
      /** `listener()` whenever `state()` changes: the permission (in System Settings) or a microphone plugged in or
       *  out; read it with `state()`. Polled every 2 s while anyone listens. Returns `stop`. */
      onChange(listener) {
        if (typeof listener !== "function") throw new TypeError("onChange(listener)");
        listeners.add(listener);
        if (timer === null) {
          poll();
          timer = win.setInterval(poll, 2000);
        }
        return () => {
          listeners.delete(listener);
          if (listeners.size === 0 && timer !== null) {
            win.clearInterval(timer);
            timer = null;
            last = null;
          }
        };
      },
    });
  }

  /** This computer's own core, the local host (docs/LOCAL_HOST.md): its state, the pairing the page uses to reach it
   *  (through the app's proxy, with this launch's secret — never the device token), the service's actions, and
   *  installing Sidevoice here (`install`, `cancel`). Each action resolves the state after it (`{state, failure?, core?,
   *  product_version, service?, calls?, attempts?, limit?, installed?, reachable}`) or rejects `{key, message}`.
   *  Only where the app offers a local host (macOS). */
  function localHost() {
    const listeners = new Set();
    let last = null; // the state last delivered, as JSON
    let timer = null;
    let localInstalls = 0; // numbers each install call's job
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
       *  one of `absent`, `installing` (while `install` runs), `not-installed`, `stopped-by-person`, `starting`,
       *  `backoff`, `running`, `failed`, `service-failed`, `refused`, `incompatible`; `product_version` the installed Sidevoice's version (the
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
      /** `{fp, public_key, device_id, token, urls: ["http://127.0.0.1:<port>"], host, local: true}`
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
      /** "Prepare this computer": installs Sidevoice from the npm registry, without Node (docs/LOCAL_HOST.md → Install;
       *  sidevoice-web's `LocalHostBridge.install`). Answers at once a promise of the state after it, once the app is
       *  paired with the new core (`LocalHostInstallOperation`), carrying `job`, this call's id for `cancel`.
       *  `onProgress({step, done, total, cancellable})` about twice a second while it runs, when that changes: `step`
       *  `download` (its bytes; `total` null when unknown), `verification`, `staging`, `service-start`, `pairing`.
       *  Rejects `{key, step?, params?, log_tail?, message}`: `install.cancelled` once cancelled, `install.busy` while
       *  another install runs, the rest as docs/BRIDGE.md lists them. */
      install(onProgress) {
        const job = "local-install-" + ++localInstalls + "-" + Date.now().toString(36);
        let finished = false;
        let told = null; // the progress last handed on, as JSON
        const operation = call("local_host_install", { job }).finally(() => { finished = true; });
        const poll = () => {
          if (finished) return;
          call("local_host_install_progress", { job })
            .then((report) => {
              if (finished || !report || typeof report !== "object" || typeof report.step !== "string") return;
              const progress = {
                step: report.step,
                done: typeof report.done === "number" ? report.done : null,
                total: typeof report.total === "number" ? report.total : null,
                cancellable: report.cancellable === true,
              };
              const key = JSON.stringify(progress);
              if (key === told) return;
              told = key;
              try { onProgress(progress); } catch (_) { /* a listener's error is the page's own */ }
            })
            .catch(() => {});
          win.setTimeout(poll, 500);
        };
        if (typeof onProgress === "function") poll();
        operation.job = job;
        return operation;
      },
      /** Stops install `job` while that is still honoured (until the package's own installer starts): true when the
       *  install will reject `install.cancelled`, false once it is too late or for a job not under way. */
      cancel: (job) =>
        typeof job === "string" && job
          ? call("local_host_install_cancel", { job }).then((accepted) => accepted === true)
          : Promise.resolve(false),
    };
  }

  /** The app's updates: the `AppUpdater` seam, 1:1 onto the `updater` command's actions (docs/BRIDGE.md → "The updater").
   *  Only in a release or a nightly that can update itself. */
  function updater() {
    const listeners = listened();
    const host = {
      settings: () => call("updater", { action: "settings" }),
      setMode: (mode) => call("updater", { action: "set-mode", mode }),
      status: () => call("updater", { action: "status" }),
      /** `listener(status)` on every change. Returns `stop`. */
      subscribe: listeners.subscribe,
      check: () => call("updater", { action: "check" }),
      download: () => call("updater", { action: "download" }),
      cancel: () => call("updater", { action: "cancel" }).then(() => undefined),
      restart: () => call("updater", { action: "restart" }).then(() => undefined),
    };
    return { host: Object.freeze(host), changed: listeners.tell };
  }

  /** Listeners the app tells of a change (`webview.eval`): `subscribe(listener)` returns `stop`; `tell(value)` hands
   *  each the value, past one that throws. */
  function listened() {
    const listeners = new Set();
    return {
      subscribe(listener) {
        if (typeof listener !== "function") throw new TypeError("subscribe(listener)");
        listeners.add(listener);
        return () => { listeners.delete(listener); };
      },
      tell(value) {
        for (const listener of listeners) {
          try { listener(value); } catch (_) { /* a listener's error is the page's own */ }
        }
      },
    };
  }

  /** The global mute shortcut: sidevoice-web's `MuteShortcutBridge`, onto the `mute_shortcut` command (docs/BRIDGE.md →
   *  "The mute shortcut"). Its `MuteShortcut` is `{accelerator, label, state, alternatives, voiceOver}`. */
  function muteShortcut() {
    const listeners = listened();
    const invalid = () => ({ key: "shortcut_invalid", message: "set(accelerator) takes a string or null." });
    const host = {
      get: () => call("mute_shortcut", { action: "get" }),
      /** `accelerator` as the web writes it (`Cmd+Shift+M`), or `null` for none. Rejects `{key: "shortcut_invalid"}`
       *  for one that is not a shortcut. */
      set: (accelerator) =>
        typeof accelerator === "string" || accelerator === null
          ? call("mute_shortcut", { action: "set", accelerator })
          : Promise.reject(invalid()),
      /** `listener(shortcut)` on every change: set here, or refused. Returns `stop`. */
      subscribe: listeners.subscribe,
    };
    return { host: Object.freeze(host), changed: listeners.tell };
  }

  /** The developer options (nightly and dev builds): sidevoice-web's hidden "Developer options" section, onto the
   *  `debug_options` command (docs/BRIDGE.md -> "The developer options"). Each switch reads live: what the app kept, then
   *  what the last `set` took. */
  function debug(initial) {
    const SWITCHES = ["inspectable", "compositingBorders", "repaintCounters"];
    const available = Object.freeze(SWITCHES.filter((name) => (initial.available || []).includes(name)));
    const state = {};
    const take = (from) => {
      for (const name of SWITCHES) state[name] = !!(from && from[name]) && available.includes(name);
    };
    take(initial);
    return Object.freeze({
      /** The switches this platform has; the page hides the others. */
      available,
      get inspectable() {
        return state.inspectable;
      },
      get compositingBorders() {
        return state.compositingBorders;
      },
      get repaintCounters() {
        return state.repaintCounters;
      },
      /** Turns one switch on or off on the room and the call controls card, at once (no reload), and keeps it. Resolves
       *  when done; the switch then reads what took. A name not in `available` or a non-boolean `on` rejects
       *  `{key: "bad_request"}`. */
      set: (name, on) =>
        available.includes(name) && typeof on === "boolean"
          ? call("debug_options", { action: "set", name, enabled: on }).then(take)
          : Promise.reject({ key: "bad_request", message: "set(name, on) takes an available switch and a boolean." }),
    });
  }

  /** The floating card's settings the app keeps: sidevoice-web's `CallControlsBridge`, onto the `call_controls_settings`
   *  command (docs/BRIDGE.md → "The call controls settings"). */
  function callControls() {
    const CORNERS = ["tl", "tr", "bl", "br"];
    return Object.freeze({
      /** `{alwaysShown, enabled, corner, moved}`: whether it shows always in a call, whether it shows at all, the corner a card never moved first appears
       *  in, and whether it was moved or resized anywhere (what `resetPlace` forgets). */
      settings: () => call("call_controls_settings", { action: "settings" }),
      /** Kept by the app and applied at once. `enabled` must be a boolean, else it rejects `{key: "bad_request"}`. */
      setAlwaysShown: (enabled) =>
        typeof enabled === "boolean"
          ? call("call_controls_settings", { action: "set-always-shown", enabled })
          : Promise.reject({ key: "bad_request", message: "setAlwaysShown(enabled) takes a boolean." }),
      /** Whether the card shows at all during a call (on by default); off, no card shows, not even the moment a change in
       *  Settings shows it. `enabled` must be a boolean, else it rejects `{key: "bad_request"}`. */
      setEnabled: (enabled) =>
        typeof enabled === "boolean"
          ? call("call_controls_settings", { action: "set-enabled", enabled })
          : Promise.reject({ key: "bad_request", message: "setEnabled(enabled) takes a boolean." }),
      /** The preferred corner: `"tl"`, `"tr"` (the default), `"bl"` or `"br"`. Setting it, the one already chosen too, moves the card there
       *  now and forgets the places it was dragged to (its sizes stay); dragged after that, it stays where it is dropped. */
      setCorner: (corner) =>
        CORNERS.includes(corner)
          ? call("call_controls_settings", { action: "set-corner", corner })
          : Promise.reject({ key: "bad_request", message: "setCorner(corner) takes tl, tr, bl or br." }),
      /** Forgets where the card was left and how the picture was resized, on every display. */
      resetPlace: () => call("call_controls_settings", { action: "reset-place" }).then(() => undefined),
      /** Forgets the picture's resized size (its "Custom") on every display, and keeps where the card was left: a
       *  starting size picked in Settings applies again. */
      clearSize: () => call("call_controls_settings", { action: "clear-size" }).then(() => undefined),
      /** During a call, shows the card a moment, in front of the room too: after a change in Settings. */
      preview: () => { invoke("call_controls_settings", { action: "preview" }); },
      /** Leaving Settings: a preview showing goes at once (else it stays while the pointer is on it or it is dragged, and goes
       *  two seconds after). */
      endPreview: () => { invoke("call_controls_settings", { action: "end-preview" }); },
      /** Whether the page shows the call's stage (the conversation's creature and its balloons): false on Settings and
       *  any other screen without it (#165). `rect`, while shown: where the stage is, in CSS pixels from the top-left
       *  corner of the page's viewport, for the app to measure how much of it other apps' windows cover. The card shows
       *  while the stage is out of view; until the page first says, the app takes it as shown. Anything else is ignored. */
      setStageVisible: (visible, rect) => {
        if (typeof visible !== "boolean") return;
        const box = rect && typeof rect === "object" && ["x", "y", "width", "height"].every((k) => Number.isFinite(rect[k]))
          ? { x: rect.x, y: rect.y, width: rect.width, height: rect.height }
          : null;
        invoke("call_controls_settings", box ? { action: "stage", visible, rect: box } : { action: "stage", visible });
      },
    });
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

  /** The call as the app shows it (tray, headset, call controls card), read from the web UI's own view model: the room's
   *  own projection for the card (`callCard`, with its `facts`), passed through as it is, with what the tray and the
   *  headset read made sure of, and what is the bridge's own on top (its version, the clock, the devices' busy). */
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
    return Object.assign({}, card, {
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
      // The room's own rows for the card, else the fields of its conversation list the card shows.
      participants: Array.isArray(card.participants) ? card.participants
        : Array.isArray(view.participants) ? view.participants.map((row) => pick(row, PARTICIPANT_FIELDS)) : [],
      devices: cardDevices(view.facts && view.facts.audioDevices),
    });
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

  // The voice's output level, for the card's avatar's mouth: during a call, at most every 40 ms, only when it changes,
  // and always the 0 that ends a reply.
  let lastPlayback = { level: 0, threadId: null };
  let lastPlaybackAt = 0;
  function reportPlayback(sample) {
    if (!reportedJoined || !sample || typeof sample !== "object") return;
    const level = Math.round(Math.max(0, Math.min(1, Number(sample.level) || 0)) * 100) / 100;
    const threadId = typeof sample.threadId === "string" ? sample.threadId : null;
    const now = Date.now();
    if (level === lastPlayback.level && threadId === lastPlayback.threadId) return;
    if (level !== 0 && now - lastPlaybackAt < 40) return;
    lastPlayback = { level, threadId };
    lastPlaybackAt = now;
    invoke("bridge_playback", { level, threadId });
  }

  let attached = false;
  function attach() {
    if (attached) return true;
    const store = win.sidevoiceUI && win.sidevoiceUI.store;
    if (!store || typeof store.subscribe !== "function" || !win.sidevoiceActions) return false;
    store.subscribe(report);
    const level = win.sidevoiceUI.micLevel;
    if (level && typeof level.subscribe === "function") level.subscribe(reportLevel);
    const playback = win.sidevoiceUI.playbackLevel;
    if (playback && typeof playback.subscribe === "function") playback.subscribe(reportPlayback);
    attached = true;
    report();
    return true;
  }

  // Only where the app runs the voice call itself (macOS): elsewhere the page runs it.
  const voiceCall = voiceOffered === true ? voice() : null;
  // Only in a release or a nightly that can update itself (core `updater::offered`).
  const updates = updaterOffered === true ? updater() : null;
  const shortcut = muteShortcut();
  // Only in a nightly or a dev build (core `debug_options::offered`): the switches' state and which this platform has.
  const debugOptions = debugState && typeof debugState === "object" && Array.isArray(debugState.available) ? debug(debugState) : null;

  const api = {
    version: BRIDGE_VERSION,
    /** What this host offers the page besides the webview itself. The web UI feature-detects it.
     *  `engine`: the app's engine, its catalogues, keys and models on disk (docs/ENGINES.md); `voice`: the call the
     *  app runs; `microphone`: the app's microphone permission, with `voice`. */
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
      // Private pairings are available on every desktop platform; machine reset is macOS-only.
      data: Object.freeze({
        pairings: Object.freeze({
          read: () => call("data_pairings_read"),
          write: (value) => call("data_pairings_write", { value }).then(() => undefined),
        }),
        ...(localHostOffered === true ? {
          deleteAll: () => call("data_delete_all"),
          uninstall: () => call("data_uninstall"),
          resetReport: () => call("data_reset_report"),
        } : {}),
      }),
      // Only where the app runs the voice call (macOS): the page then neither opens the microphone nor runs models.
      ...(voiceCall ? { voice: voiceCall.host } : {}),
      // With `voice`: the app's microphone permission, which the page cannot ask for itself there.
      ...(voiceCall ? { microphone: microphone() } : {}),
      /** What this build is: a release, a nightly (both with `updater`), or a development build. */
      build: Object.freeze({ ...build }),
      // The name the person gave this computer ("Mac de Ana"): the page proposes it as the device's name. Only
      // where the system has one.
      ...(typeof computerName === "string" && computerName ? { computerName } : {}),
      /** The global mute shortcut, and whether the call controls card shows always in a call: the app's, set from
       *  the page's Settings. */
      muteShortcut: shortcut.host,
      callControls: callControls(),
      // Only in a nightly or a dev build: the developer options (docs/BRIDGE.md -> "The developer options").
      ...(debugOptions ? { debug: debugOptions } : {}),
      // Only in a release or a nightly that can update itself.
      ...(updates ? { updater: updates.host } : {}),
    }),
    /** An event of the voice call, from the app (`webview.eval`): to `host.voice`'s listeners. */
    voiceEvent(event) {
      if (voiceCall) voiceCall.receive(event);
    },
    /** The updates' status changed, from the app (`webview.eval`): to `host.updater.subscribe`'s listeners. */
    updaterEvent(status) {
      if (updates) updates.changed(status);
    },
    /** The mute shortcut changed, from the app (`webview.eval`): to `host.muteShortcut.subscribe`'s listeners. */
    muteShortcutEvent(status) {
      shortcut.changed(status);
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
          // Only a conversation the room lists. One whose agent is away is one the call can go to: what is said to it
          // waits there for its agent, as the room's own pick allows (sidevoice-web selectParticipant).
          if (!state.participants.some((row) => row.threadId === request.threadId)) return false;
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
        case "open-settings":
          // The card's gear: the room's Settings at the floating card's section (the app brings the room in front).
          if (!win.sidevoiceUI || typeof win.sidevoiceUI.openSettings !== "function") return false;
          win.sidevoiceUI.openSettings("floating-card");
          return true;
        case "show-settings":
          // The tray's Settings: the room's Settings as it was left (the app brings the room in front).
          if (!win.sidevoiceUI || typeof win.sidevoiceUI.openSettings !== "function") return false;
          win.sidevoiceUI.openSettings();
          return true;
        case "keep-alive":
          // The idle warning's Keep on the card: a sign of the person, so the call stays on.
          if (typeof actions.keepVoiceOn !== "function") return false;
          actions.keepVoiceOn();
          return true;
        case "cancel-input":
          // The room's "Cancel sending": takes back the person's turn while it is recorded or transcribed.
          if (typeof actions.cancelInput !== "function") return false;
          settle(actions.cancelInput());
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
