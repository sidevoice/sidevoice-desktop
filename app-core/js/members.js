// The members of the page's `host` that every Sidevoice app shares, whatever shell it is made with
// (sidevoice-app-core): the app's engine, the voice call it runs, the microphone's permission and the pairings, and
// the room page's call state reported to the app, with the commands the app runs on it. They name no shell: what
// reaches the app is a `transport`, handed in, that has the shape below (the shell's binding makes it).
//
//   transport.call(command, args)   the shared command, as a promise
//   transport.channel(onmessage)    what a command that takes a channel is given in its place; the app's messages over it
//                                   are handed to `onmessage` in the order the app made them
//
// (A transport has `send(command, args)` too, the same as `call` without an answer and never an error; the members have no
// use for it, the apps that report levels do.)
//
// This file is one function expression, `(win, transport) => members`, which the app evaluates (docs/BRIDGE.md ->
// "Parts"). Each member is made when it is asked for: an app offers the ones it has.
(function (win, transport) {
  "use strict";
  const { call, channel } = transport;

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
   *  sends, through a handle that tells how it went. A refusal rejects with `{key, message, …params}`. Offered on every
   *  OS (core `bridge`); the factory keeps the flag while the page reads it. */
  /** Native implementation of sidevoice-voice's VoiceHost. The host owns one microphone, speaker and model pool;
   *  `start(settings)` makes a call with an input half, an output half or both, which owns its events, playback and
   *  recognition state, and `preload(settings)` loads the models a call of them would use. */
  function voice() {
    const calls = new Map();
    // The app numbers the pages that own its calls and sends a page's events over the channel it opens when it loads, to that
    // page only. Every command carries the number back, so that a page that was replaced moves nothing of the one that
    // replaced it. Asked once, with the first command; asked again if the app refused it.
    let voiceApi;
    let numbered = null;
    const toApp = (command, args) => {
      numbered ??= Promise.resolve().then(() => call("voice_events", { events: channel((event) => voiceApi?.receive(event)) }));
      return numbered.then((page) => call(command, { page, ...args }), (error) => {
        numbered = null;
        throw error;
      });
    };
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
      const created = toApp("voice_create_call", { key: id });
      const installed = created.then(() => toApp("voice_start", { key: id, settings, take: options?.take === true }));
      // It works when the app says so and the page has the state that says it (the app's events and its answers reach the
      // page by different ways, and a page that hears `ready` must find the state it comes with).
      let sawWorking;
      let failWorking;
      const working = new Promise((resolve, reject) => { sawWorking = resolve; failWorking = reject; });
      working.catch(() => {});
      const ready = installed.then(() => toApp("voice_ready", { key: id })).then(() => working);
      // The page reads `ready`; one it never reads is no unhandled rejection of the bridge's.
      ready.catch(() => {});
      const asked = (after) => (command, args) => after.then(() => {
        if (closing) throw closed();
        return toApp(command, { key: id, ...args });
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
          cancel: () => { asking.then(() => sayings.has(key) && toApp("voice_cancel_say", { key: id, sayKey: key })).catch(() => {}); },
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
        return toApp("voice_stop", { key: id }).then((ran) => {
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
          await toApp("voice_dispose_call", { key: id });
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
        return toApp("voice_preload", { settings });
      },
      routeInput(target) {
        if (target !== null && (!target || calls.get(target.id)?.host !== target)) {
          return Promise.reject(new TypeError("a call from this voice host or null"));
        }
        route = route.catch(() => {}).then(async () => {
          if (target) await calls.get(target.id).created;
          return toApp("voice_route_input", { key: target?.id ?? null });
        });
        return route;
      },
      audioDevices: () => toApp("voice_audio_devices"),
      setAudioDevices: (choice) => {
        const chosen = {};
        for (const kind of ["input", "output"]) {
          const id = choice && choice[kind];
          if (id !== undefined) chosen[kind] = id === null ? null : String(id);
        }
        return toApp("voice_set_audio_devices", { choice: chosen });
      },
      onAudioDevices: on(deviceListeners),
    });
    voiceApi = { host, receive(event) {
      if (!event || typeof event !== "object") return;
      if (event.type === "devices") dispatch(deviceListeners, event.data);
      else calls.get(event.key)?.receive(event);
    } };
    return voiceApi;
  }

  /** The app's microphone access (docs/BRIDGE.md → "The microphone permission"), onto the `microphone` command, on every
   *  OS: the system's own answer, never the webview's. The app runs the voice call, so the page is never granted
   *  `getUserMedia`: this is also how it asks the OS for the microphone. The first-run setup asks through this.
   *  `settingsPage`: whether the OS has a microphone privacy page to open (`openSettings`). */
  function microphone(settingsPage) {
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
      /** macOS's own prompt the first time; afterwards, and on every other OS, the same as `state()` (there is no prompt to
       *  wait for), so any of `state()`'s answers. */
      request: () => call("microphone", { action: "request" }),
      /** Opens the system's microphone privacy page, where a denial is undone. Only where the OS has one (not Linux). */
      ...(settingsPage === true ? {
        openSettings: () => call("microphone", { action: "open-settings" }).then(() => undefined),
      } : {}),
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

  /** The pairings the app keeps for the page (docs/BRIDGE.md -> "Pairings"): the page reads and writes them whole. */
  function pairings() {
    return Object.freeze({
      read: () => call("pairings_read"),
      write: (value) => call("pairings_write", { value }).then(() => undefined),
    });
  }

  /** The room page and the app, the call state one way and the commands the other (docs/BRIDGE.md -> "The call state").
   *  The page reports a snapshot of the call as the room's own view model says it, whenever it changes, to `publish`;
   *  `run(request)` runs a command of the app (from a tray, a shortcut, a headset, a notification) through the web UI's own actions.
   *  Only the room's seams are read (`window.sidevoiceUI.store` and `window.sidevoiceActions`); the DOM never is.
   *
   *  The shared commands are `toggle-mute`, `hang-up`, `skip-reply`, `keep-alive` and `cancel-input`. What else an app
   *  shows of the call, and what else it can ask the room, it adds:
   *   - `version`: the snapshot's version, stamped in every snapshot.
   *   - `publish(snapshot)`: where each snapshot that changed goes.
   *   - `extend(view, card)`: the fields the app adds to a snapshot, from the room's whole view and its `callCard`
   *     projection; `view` is null while the room is not up, and the fields then are the ones of an empty call.
   *   - `commands`: more commands, `name(request, state, actions) => boolean` (whether it ran).
   *   - `attached(ui)`: once, when the room is up, to subscribe to the room's other seams.
   *  Returns `{snapshot, report, run, joined, start}`; `joined()` is whether the last snapshot reported a call. */
  function room({ version, publish, extend, commands = {}, attached }) {
    let lastReported = "";
    let reportedJoined = false;
    /** When the call being reported was joined (ms since the epoch), for the app's clock; null outside a call. */
    let joinedAt = null;

    /** The call as the app shows it (in a tray, on a headset, in a notification), read from the web UI's own view model: the room's
     *  own projection (`callCard`, with its `facts`), passed through as it is, with what those surfaces read
     *  made sure of, and what is the bridge's own on top (its version, the clock, the app's fields). */
    function snapshot() {
      const store = win.sidevoiceUI && win.sidevoiceUI.store;
      const actions = win.sidevoiceActions;
      if (!store || !actions) {
        return { version, ready: false, joined: false, busy: false, micEnabled: true, micDisabled: false, title: "",
          agent: "idle", youTalking: false, canSkip: false, since: null, ...extend(null, {}) };
      }
      const view = store.getState() || {};
      const session = view.call || {};
      const mic = view.mic || {};
      // The room's own view of the call, projected by the room from its facts (state/room-session-state.js callCardView): the agent
      // independent of the person, and the conversation the call is on.
      const card = view.callCard || {};
      const joined = !!session.joined;
      if (!joined) joinedAt = null;
      else if (joinedAt === null) joinedAt = Date.now();
      return Object.assign({}, card, {
        version,
        ready: true,
        joined,
        busy: !!session.busy,
        micEnabled: mic.enabled !== false,
        micDisabled: !!mic.disabled,
        title: typeof card.title === "string" ? card.title : "",
        agent: ["idle", "working", "speaking"].includes(card.agent) ? card.agent : "idle",
        youTalking: !!card.youTalking,
        canSkip: !!card.canSkip,
        since: joinedAt,
      }, extend(view, card));
    }

    function report() {
      const state = snapshot();
      const key = JSON.stringify(state);
      if (key === lastReported) return; // the store ticks often (clocks, meters); the app only cares about changes
      lastReported = key;
      reportedJoined = state.joined;
      publish(state);
    }

    function run(request) {
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
        case "keep-alive":
          // The idle warning's Keep: a sign of the person, so the call stays on.
          if (typeof actions.keepVoiceOn !== "function") return false;
          actions.keepVoiceOn();
          return true;
        case "cancel-input":
          // The room's "Cancel sending": takes back the person's turn while it is recorded or transcribed.
          if (typeof actions.cancelInput !== "function") return false;
          settle(actions.cancelInput());
          return true;
        default:
          return Object.prototype.hasOwnProperty.call(commands, request.command)
            && commands[request.command](request, state, actions) === true;
      }
    }

    let subscribed = false;
    function subscribe() {
      if (subscribed) return true;
      const ui = win.sidevoiceUI;
      const store = ui && ui.store;
      if (!store || typeof store.subscribe !== "function" || !win.sidevoiceActions) return false;
      store.subscribe(report);
      if (typeof attached === "function") attached(ui);
      subscribed = true;
      report();
      return true;
    }

    /** Reports "not ready yet" (the app's surfaces grey out until the room's controller exists), then follows the room: it builds
     *  its store and actions after its own scripts load, so this waits for them. */
    function start() {
      report();
      if (!subscribe()) {
        const timer = win.setInterval(() => {
          if (subscribe()) win.clearInterval(timer);
        }, 250);
      }
    }

    return Object.freeze({ snapshot, report, run, joined: () => reportedJoined, start });
  }

  return Object.freeze({ engine, voice, microphone, pairings, room });
})
